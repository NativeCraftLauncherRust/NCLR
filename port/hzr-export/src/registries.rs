//! Rebuild Horizon's `command-registries.json` from the player's own `client.jar`.
//!
//! `horizon-prepare` runs `VanillaCommandParseOracle` (`registries` mode) against the
//! real 26.3 jars and writes the result as `command-registries.json`. That table is
//! mostly *data* the player already owns (`data/minecraft/**` inside their jar) plus
//! a handful of registries whose entries only exist as bytecode in `net/minecraft`.
//! This module reads both: the jar's data pack for the data-driven part, and the
//! class files for the code-driven part, so no value comes from `ship.hzr3`.
//!
//! What it produces, and what it cannot:
//!
//! * exact — `learnable_recipes`, `advancement_criteria`, `block_tag_members`,
//!   `block_tags`, `item_tags`, `datapacks_{selected,available,enableable}`,
//!   `clock_time_markers`, `templates`, `enchantments`, `structures`, `biomes`,
//!   `damage_types`, `clocks`, `timelines`, `dimensions`, `players`,
//!   `selected_entities`, `teams`, `objectives`, `tracked`, `sounds`, `effects`,
//!   `attributes`, `entity_types`, `summonable`, `persistent_components`, and the
//!   52 `by_registry` registries the jar's data pack (or the class files) name.
//! * not reproducible from the client jar alone — `items` (registry *order* comes
//!   from `Items.<clinit>`'s loops over colour collections, not from data),
//!   `blocks` (a block's state properties are built in its constructor; the
//!   `blockstates/` assets only list the values a model happens to name),
//!   `block_entities` (needs `Block#hasBlockEntity` per block *in registry order*,
//!   and the colour-collection blocks are registered inside loops), and the
//!   code-driven half of `by_registry`.
//!
//! The other four tables this task names are out of reach from the jar without
//! running vanilla's code: `items.json` is per-item component data built by item
//! constructors, `commands.json` is the Brigadier tree built by `Commands.<init>`,
//! `command-registries.json` is this file (written here), and
//! `creative-inventory.json` is `CreativeModeTabs`' contents.
//!
//! `classfile.rs` is shared with `hzr-bytecode` rather than copied: the crate
//! cannot depend on it (a manifest edit is out of scope), so it is included by
//! path.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;

use serde_json::{Map, Value};

#[path = "../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;
use classfile::ClassFile;

/// Every top-level key `VanillaCommandParseOracle.exportRegistries` writes.
pub const KEYS: [&str; 32] = [
    "learnable_recipes",
    "advancement_criteria",
    "block_entities",
    "persistent_components",
    "block_tag_members",
    "datapacks_selected",
    "datapacks_available",
    "datapacks_enableable",
    "clock_time_markers",
    "templates",
    "items",
    "blocks",
    "item_tags",
    "block_tags",
    "entity_types",
    "summonable",
    "effects",
    "attributes",
    "sounds",
    "enchantments",
    "structures",
    "biomes",
    "damage_types",
    "clocks",
    "timelines",
    "dimensions",
    "players",
    "selected_entities",
    "teams",
    "objectives",
    "tracked",
    "by_registry",
];

/// What `ship.hzr3` calls this table.
pub const TABLE: &str = "command-registries.json";

/// The tables of the same slice this module cannot build, with the reason.
///
/// `registries.json` and `items.json` are not oracle outputs at all: `horizon-prepare`
/// takes them from vanilla's **data generator** report (`reports/registries.json`,
/// `reports/minecraft/components/item/*`), which is only produced by running the
/// generator's JVM. `commands.json` is the Brigadier tree `Commands.<init>` builds.
pub const BLOCKED_TABLES: [(&str, &str); 4] = [
    (
        "registries.json",
        "vanilla's data generator report (`reports/registries.json`: every registry's \
         protocol ids, defaults and entries) — needs the generator's JVM",
    ),
    (
        "items.json",
        "the data generator's per-item component report (`reports/minecraft/components/item`) \
         — needs the generator's JVM",
    ),
    (
        "commands.json",
        "the Brigadier tree is built by `Commands.<init>`/the command classes at runtime",
    ),
    (
        "creative-inventory.json",
        "creative tabs are populated by `CreativeModeTabs` at runtime",
    ),
];

/// Which of [`KEYS`] the jar allowed this build to reconstruct, and why the rest
/// are absent.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub produced: Vec<String>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
}

impl Coverage {
    pub fn empty() -> Self {
        Coverage {
            produced: Vec::new(),
            missing: KEYS.iter().map(|key| (*key).to_string()).collect(),
            notes: Vec::new(),
        }
    }

    fn add(&mut self, key: &str) {
        self.produced.push(key.to_string());
        self.missing.retain(|name| name != key);
    }

    fn note(&mut self, note: &str) {
        self.notes.push(note.to_string());
    }
}

/// The player's `client.jar`, held in memory.
pub struct Jar {
    files: BTreeMap<String, Vec<u8>>,
    classes: std::cell::RefCell<BTreeMap<String, std::rc::Rc<ClassFile>>>,
}

impl Jar {
    pub fn open(path: &Path) -> Result<Self, String> {
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
            classes: std::cell::RefCell::new(BTreeMap::new()),
        })
    }

    fn bytes(&self, name: &str) -> Result<&[u8], String> {
        self.files
            .get(name)
            .map(|bytes| bytes.as_slice())
            .ok_or_else(|| format!("{name}: not in the jar"))
    }

    fn json(&self, name: &str) -> Result<Value, String> {
        serde_json::from_slice(self.bytes(name)?).map_err(|error| format!("{name}: {error}"))
    }

    /// Parse (and cache) one class, addressed by its internal name.
    fn class(&self, internal: &str) -> Result<std::rc::Rc<ClassFile>, String> {
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

    /// Every `prefix**suffix` entry: the id it names (`minecraft:`-prefixed) and
    /// the jar path, in path order.
    fn entries(&self, prefix: &str, suffix: &str) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .files
            .keys()
            .filter_map(|name| {
                let rest = name.strip_prefix(prefix)?;
                let id = rest.strip_suffix(suffix)?;
                if id.is_empty() {
                    return None;
                }
                Some((format!("minecraft:{id}"), name.clone()))
            })
            .collect();
        out.sort();
        out
    }

    /// The ids of every entry in `prefix`, sorted.
    fn ids(&self, prefix: &str, suffix: &str) -> Vec<Value> {
        self.entries(prefix, suffix)
            .into_iter()
            .map(|(id, _)| Value::String(id))
            .collect()
    }

    /// Immediate child directory names of `prefix`.
    fn child_dirs(&self, prefix: &str) -> Vec<String> {
        let mut out = BTreeSet::new();
        for name in self.files.keys() {
            let Some(rest) = name.strip_prefix(prefix) else {
                continue;
            };
            if let Some((dir, _)) = rest.split_once('/') {
                if !dir.is_empty() {
                    out.insert(dir.to_string());
                }
            }
        }
        out.into_iter().collect()
    }
}

// ---------------------------------------------------------------------------
// Bytecode scanning
// ---------------------------------------------------------------------------

/// One `putstatic`-delimited run of instructions in a static initializer.
///
/// Vanilla's registry classes all follow the same shape: build a value, register
/// it, store it in a field. Splitting a `<clinit>` on `putstatic` therefore cuts
/// it into one segment per registration, inside which the id string and the field
/// being written are unambiguous — no operand-stack simulation needed.
#[derive(Debug, Default, Clone)]
struct Segment {
    /// The first string constant of the segment; for every registry class that
    /// is the entry's id.
    first_string: Option<String>,
    /// The first static field read; for `EntityTypes.<clinit>` that is
    /// `EntityTypeIds.<NAME>`.
    first_field: Option<(String, String)>,
    /// The field this segment's `putstatic` writes.
    field: Option<(String, String)>,
    /// A `register*` helper is called in this segment.
    registers: bool,
    /// `EntityType.Builder#noSummon` is called in this segment.
    no_summon: bool,
}

impl Segment {
    fn is_empty(&self) -> bool {
        self.first_string.is_none()
            && self.first_field.is_none()
            && self.field.is_none()
            && !self.registers
            && !self.no_summon
    }
}

fn u16_at(code: &[u8], at: usize) -> Result<usize, String> {
    if at + 2 > code.len() {
        return Err(format!("bytecode ends inside an operand at {at}"));
    }
    Ok(((code[at] as usize) << 8) | code[at + 1] as usize)
}

/// The length in bytes of the instruction starting at `at`.
///
/// Only the shapes matter here, not their meaning; `tableswitch`/`lookupswitch`
/// and `wide` need their own arithmetic because their length depends on the
/// alignment and on an operand.
fn instruction_len(code: &[u8], at: usize) -> Result<usize, String> {
    let opcode = *code
        .get(at)
        .ok_or_else(|| format!("bytecode ends at {at}"))?;
    let length = match opcode {
        0x00..=0x0f => 1,
        0x10 => 2,
        0x11 => 3,
        0x12 => 2,
        0x13 | 0x14 => 3,
        0x15..=0x19 => 2,
        0x1a..=0x35 => 1,
        0x36..=0x3a => 2,
        0x3b..=0x83 => 1,
        0x84 => 3,
        0x85..=0x98 => 1,
        0x99..=0xa8 => 3,
        0xa9 => 2,
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
        0xac..=0xb1 => 1,
        0xb2..=0xb8 => 3,
        0xb9 | 0xba => 5,
        0xbb => 3,
        0xbc => 2,
        0xbd => 3,
        0xbe | 0xbf => 1,
        0xc0 | 0xc1 => 3,
        0xc2 | 0xc3 => 1,
        0xc4 => {
            let next = *code.get(at + 1).ok_or("wide out of range")?;
            if next == 0x84 {
                6
            } else {
                4
            }
        }
        0xc5 => 4,
        0xc6 | 0xc7 => 3,
        0xc8 | 0xc9 => 5,
        other => return Err(format!("unmodelled opcode {other:#04x} at {at}")),
    };
    if at + length > code.len() {
        return Err(format!("instruction at {at} runs past the code"));
    }
    Ok(length)
}

fn method_code<'a>(class: &'a ClassFile, method: &str) -> Option<&'a [u8]> {
    class
        .method(method)
        .and_then(|member| member.code.as_ref())
        .map(|code| code.bytes.as_slice())
}

/// Cut `class.<method>` into one [`Segment`] per `putstatic`.
fn segments(class: &ClassFile, method: &str) -> Result<Vec<Segment>, String> {
    let Some(code) = method_code(class, method) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    let mut current = Segment::default();
    let mut at = 0usize;
    while at < code.len() {
        let opcode = code[at];
        let length = instruction_len(code, at)?;
        match opcode {
            // ldc / ldc_w: a string constant is the only value worth keeping.
            0x12 => {
                let index = code[at + 1] as usize;
                if current.first_string.is_none() {
                    current.first_string = classfile::string_constant(&class.constants, index);
                }
            }
            0x13 => {
                let index = u16_at(code, at + 1)?;
                if current.first_string.is_none() {
                    current.first_string = classfile::string_constant(&class.constants, index);
                }
            }
            // getstatic: remember the first field read.
            0xb2 => {
                let index = u16_at(code, at + 1)?;
                if current.first_field.is_none() {
                    current.first_field = classfile::field_ref(&class.constants, index)
                        .map(|(owner, name, _)| (owner, name));
                }
            }
            // putstatic: the segment ends.
            0xb3 => {
                let index = u16_at(code, at + 1)?;
                current.field = classfile::field_ref(&class.constants, index)
                    .map(|(owner, name, _)| (owner, name));
                out.push(std::mem::take(&mut current));
            }
            // invokevirtual: `noSummon` marks an entity type nobody can /summon.
            0xb6 => {
                let index = u16_at(code, at + 1)?;
                if let Some((_, name, _)) = classfile::method_ref(&class.constants, index) {
                    if name == "noSummon" {
                        current.no_summon = true;
                    }
                }
            }
            // invokestatic: a `register*` helper names the registry entry being added.
            0xb8 => {
                let index = u16_at(code, at + 1)?;
                if let Some((_, name, _)) = classfile::method_ref(&class.constants, index) {
                    if name.starts_with("register") {
                        current.registers = true;
                    }
                }
            }
            _ => {}
        }
        at += length;
    }
    if !current.is_empty() {
        out.push(current);
    }
    Ok(out)
}

/// `field -> id string`, for the `*Ids` reference classes
/// (`BlockItemIds`, `ItemIds`, `EntityTypeIds`): `<clinit>` is `ldc "name";
/// create(name); putstatic FIELD`.
fn field_ids(class: &ClassFile, method: &str) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for segment in segments(class, method)? {
        if let (Some((_, field)), Some(id)) = (segment.field, segment.first_string) {
            out.insert(field, id);
        }
    }
    Ok(out)
}

/// The ids a registry class registers with `register(...)` calls, sorted.
///
/// `SoundEvents`, `MobEffects` and `Attributes` all store the id as the first
/// string constant of the segment that registers the entry.
fn registered_ids(class: &ClassFile, method: &str) -> Result<BTreeSet<String>, String> {
    let mut out = BTreeSet::new();
    for segment in segments(class, method)? {
        if let (true, Some(id)) = (segment.registers, segment.first_string) {
            out.insert(format!("minecraft:{id}"));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Data-pack tables
// ---------------------------------------------------------------------------

/// The recipe types whose recipes a player can learn from the recipe book.
///
/// Vanilla's `Recipe#isSpecial` is true for the dynamic/special serializers
/// (`crafting_special_*`), for brewing, and for `crafting_decorated_pot`; every
/// other type in `data/minecraft/recipe/` is learnable. Measured against a
/// Java-produced table: 2,042 files − 303 non-learnable = 1,739.
const LEARNABLE_RECIPE_TYPES: [&str; 12] = [
    "minecraft:crafting_shaped",
    "minecraft:crafting_shapeless",
    "minecraft:crafting_transmute",
    "minecraft:crafting_dye",
    "minecraft:crafting_imbue",
    "minecraft:stonecutting",
    "minecraft:smelting",
    "minecraft:blasting",
    "minecraft:smoking",
    "minecraft:campfire_cooking",
    "minecraft:smithing_transform",
    "minecraft:smithing_trim",
];

fn learnable_recipes(jar: &Jar) -> Result<Value, String> {
    let mut ids = Vec::new();
    for (id, path) in jar.entries("data/minecraft/recipe/", ".json") {
        let recipe = jar.json(&path)?;
        let kind = recipe
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("minecraft:crafting_shaped");
        if LEARNABLE_RECIPE_TYPES.contains(&kind) {
            ids.push(id);
        }
    }
    ids.sort();
    Ok(Value::Array(ids.into_iter().map(Value::String).collect()))
}

fn advancement_criteria(jar: &Jar) -> Result<Value, String> {
    let mut out = Map::new();
    for (id, path) in jar.entries("data/minecraft/advancement/", ".json") {
        let advancement = jar.json(&path)?;
        let mut names: Vec<String> = advancement
            .get("criteria")
            .and_then(Value::as_object)
            .map(|criteria| criteria.keys().cloned().collect())
            .unwrap_or_default();
        names.sort();
        out.insert(
            id,
            Value::Array(names.into_iter().map(Value::String).collect()),
        );
    }
    Ok(Value::Object(out))
}

/// One tag file's `values` list: `(id, is_reference)` in file order.
fn tag_values(jar: &Jar, path: &str) -> Result<Vec<(String, bool)>, String> {
    let document = jar.json(path)?;
    let mut out = Vec::new();
    let Some(values) = document.get("values").and_then(Value::as_array) else {
        return Ok(out);
    };
    for value in values {
        match value {
            Value::String(text) => match text.strip_prefix('#') {
                Some(target) => out.push((target.to_string(), true)),
                None => out.push((text.clone(), false)),
            },
            Value::Object(entry) => {
                if let Some(text) = entry.get("id").and_then(Value::as_str) {
                    match text.strip_prefix('#') {
                        Some(target) => out.push((target.to_string(), true)),
                        None => out.push((text.to_string(), false)),
                    }
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// All tag files of one tag directory, expanded through `#` references.
///
/// Vanilla's `TagLoader` appends the referenced tag's members where the reference
/// sits, so the order here is the order a loaded `Tag` iterates.
fn expanded_tags(jar: &Jar, directory: &str) -> Result<BTreeMap<String, Vec<String>>, String> {
    let prefix = format!("data/minecraft/tags/{directory}/");
    let mut files: BTreeMap<String, Vec<(String, bool)>> = BTreeMap::new();
    for (id, path) in jar.entries(&prefix, ".json") {
        files.insert(id, tag_values(jar, &path)?);
    }
    let mut resolved: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for id in files.keys() {
        let mut visiting = BTreeSet::new();
        let members = resolve_tag(id, &files, &mut resolved, &mut visiting)?;
        resolved.insert(id.clone(), members);
    }
    Ok(resolved)
}

fn resolve_tag(
    id: &str,
    files: &BTreeMap<String, Vec<(String, bool)>>,
    resolved: &mut BTreeMap<String, Vec<String>>,
    visiting: &mut BTreeSet<String>,
) -> Result<Vec<String>, String> {
    if let Some(done) = resolved.get(id) {
        return Ok(done.clone());
    }
    if !visiting.insert(id.to_string()) {
        return Err(format!("tag {id} includes itself"));
    }
    let mut members: Vec<String> = Vec::new();
    if let Some(values) = files.get(id) {
        for (value, reference) in values {
            if *reference {
                let inner = resolve_tag(value, files, resolved, visiting)?;
                for member in inner {
                    if !members.contains(&member) {
                        members.push(member);
                    }
                }
            } else if !members.contains(value) {
                members.push(value.clone());
            }
        }
    }
    visiting.remove(id);
    Ok(members)
}

fn tags_of(jar: &Jar, directory: &str) -> Vec<Value> {
    jar.entries(&format!("data/minecraft/tags/{directory}/"), ".json")
        .into_iter()
        .map(|(id, _)| Value::String(id))
        .collect()
}

/// The clocks' command-visible time markers, from the timelines' `time_markers`.
fn clock_time_markers(jar: &Jar) -> Result<Value, String> {
    let mut markers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (clock, _) in jar.entries("data/minecraft/world_clock/", ".json") {
        markers.entry(clock).or_default();
    }
    for (_, path) in jar.entries("data/minecraft/timeline/", ".json") {
        let timeline = jar.json(&path)?;
        let Some(clock) = timeline.get("clock").and_then(Value::as_str) else {
            continue;
        };
        let mut clock = clock.to_string();
        if !clock.contains(':') {
            clock = format!("minecraft:{clock}");
        }
        let entry = markers.entry(clock).or_default();
        let Some(names) = timeline.get("time_markers").and_then(Value::as_object) else {
            continue;
        };
        for (name, marker) in names {
            let shown = marker
                .get("show_in_commands")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if shown {
                entry.insert(name.clone());
            }
        }
    }
    let mut out = Map::new();
    for (clock, names) in markers {
        out.insert(
            clock,
            Value::Array(names.into_iter().map(Value::String).collect()),
        );
    }
    Ok(Value::Object(out))
}

/// The built-in data packs the server's repository offers.
///
/// `vanilla` is always there; the three experiment packs live in the jar under
/// `data/minecraft/datapacks/`. A pack whose `pack.mcmeta` enables feature flags
/// cannot be enabled with `FeatureFlags.VANILLA_SET`, which is why only `vanilla`
/// is enableable.
fn datapacks(jar: &Jar) -> Result<(Value, Value, Value), String> {
    let mut available: Vec<String> = jar.child_dirs("data/minecraft/datapacks/");
    available.push("vanilla".to_string());
    available.sort();
    let mut enableable = Vec::new();
    for pack in &available {
        let meta = format!("data/minecraft/datapacks/{pack}/pack.mcmeta");
        let enabled_features = jar
            .files
            .contains_key(&meta)
            .then(|| jar.json(&meta))
            .transpose()?
            .and_then(|document| {
                document
                    .get("features")
                    .and_then(|features| features.get("enabled"))
                    .and_then(Value::as_array)
                    .map(|enabled| enabled.len())
            })
            .unwrap_or(0);
        if enabled_features == 0 {
            enableable.push(pack.clone());
        }
    }
    let selected = vec!["vanilla".to_string()];
    let as_values = |list: Vec<String>| Value::Array(list.into_iter().map(Value::String).collect());
    Ok((
        as_values(selected),
        as_values(available),
        as_values(enableable),
    ))
}

fn string_array(items: &[&str]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|item| Value::String((*item).to_string()))
            .collect(),
    )
}

/// The literals `VanillaCommandParseOracle` puts into the source stack it parses
/// against. They are the oracle's own fixtures, not game data, so they are copied
/// verbatim rather than derived.
fn source_fixtures() -> [(&'static str, Value); 6] {
    [
        (
            "dimensions",
            string_array(&[
                "minecraft:overworld",
                "minecraft:the_nether",
                "minecraft:the_end",
            ]),
        ),
        ("players", string_array(&["Player", "Steve", "Alex"])),
        (
            "selected_entities",
            string_array(&["0b9c6a7e-3d1f-4a2b-9c8d-7e6f5a4b3c2d"]),
        ),
        ("teams", string_array(&["red", "blue"])),
        ("objectives", string_array(&["kills", "health", "obj"])),
        ("tracked", string_array(&["#total", "Steve", "fake"])),
    ]
}

// ---------------------------------------------------------------------------
// `by_registry`
// ---------------------------------------------------------------------------

/// Data directories whose contents are one registry, by registry id.
const DATA_REGISTRIES: [(&str, &str); 30] = [
    ("advancement", "minecraft:advancement"),
    ("banner_pattern", "minecraft:banner_pattern"),
    ("cat_sound_variant", "minecraft:cat_sound_variant"),
    ("cat_variant", "minecraft:cat_variant"),
    ("chicken_sound_variant", "minecraft:chicken_sound_variant"),
    ("chicken_variant", "minecraft:chicken_variant"),
    ("cow_sound_variant", "minecraft:cow_sound_variant"),
    ("cow_variant", "minecraft:cow_variant"),
    ("damage_type", "minecraft:damage_type"),
    ("dialog", "minecraft:dialog"),
    ("enchantment", "minecraft:enchantment"),
    ("enchantment_provider", "minecraft:enchantment_provider"),
    ("frog_variant", "minecraft:frog_variant"),
    ("instrument", "minecraft:instrument"),
    ("item_modifier", "minecraft:item_modifier"),
    ("jukebox_song", "minecraft:jukebox_song"),
    ("loot_table", "minecraft:loot_table"),
    ("painting_variant", "minecraft:painting_variant"),
    ("pig_sound_variant", "minecraft:pig_sound_variant"),
    ("pig_variant", "minecraft:pig_variant"),
    ("predicate", "minecraft:predicate"),
    ("recipe", "minecraft:recipe"),
    ("test_environment", "minecraft:test_environment"),
    ("test_instance", "minecraft:test_instance"),
    ("trade_set", "minecraft:trade_set"),
    ("trim_material", "minecraft:trim_material"),
    ("trim_pattern", "minecraft:trim_pattern"),
    ("villager_trade", "minecraft:villager_trade"),
    ("wolf_sound_variant", "minecraft:wolf_sound_variant"),
    ("wolf_variant", "minecraft:wolf_variant"),
];

fn by_registry(jar: &Jar, code_registries: &BTreeMap<&str, Vec<String>>) -> Result<Value, String> {
    let mut out = Map::new();
    let insert = |maybe_out: &mut Map<String, Value>, id: &str, ids: Vec<String>| {
        let directory = id.strip_prefix("minecraft:").unwrap_or(id);
        let mut unique: BTreeSet<String> = BTreeSet::new();
        unique.extend(ids);
        let entry = serde_json::json!({
            "ids": unique.into_iter().map(Value::String).collect::<Vec<_>>(),
            "tags": tags_of(jar, directory),
        });
        maybe_out.insert(id.to_string(), entry);
    };
    for (directory, id) in DATA_REGISTRIES {
        let prefix = format!("data/minecraft/{directory}/");
        let ids: Vec<String> = jar
            .entries(&prefix, ".json")
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        insert(&mut out, id, ids);
    }
    // `data/minecraft/worldgen/<kind>/` names the registry `minecraft:worldgen/<kind>`.
    for kind in jar.child_dirs("data/minecraft/worldgen/") {
        let prefix = format!("data/minecraft/worldgen/{kind}/");
        let ids: Vec<String> = jar
            .entries(&prefix, ".json")
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        insert(&mut out, &format!("minecraft:worldgen/{kind}"), ids);
    }
    for (id, ids) in code_registries {
        insert(&mut out, id, ids.clone());
    }
    Ok(Value::Object(out))
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// The `command-registries.json` document this jar can reproduce, plus what it
/// had to leave out.
pub fn command_registries(jar_path: &Path) -> Result<(Value, Coverage), String> {
    let jar = Jar::open(jar_path)?;
    let mut out = Map::new();
    let mut coverage = Coverage::empty();

    let put = |out: &mut Map<String, Value>, coverage: &mut Coverage, key: &str, value: Value| {
        out.insert(key.to_string(), value);
        coverage.add(key);
    };

    put(
        &mut out,
        &mut coverage,
        "learnable_recipes",
        learnable_recipes(&jar)?,
    );
    put(
        &mut out,
        &mut coverage,
        "advancement_criteria",
        advancement_criteria(&jar)?,
    );

    // Tags: ids, members (blocks), and the two registries' tag lists.
    let block_tag_members = expanded_tags(&jar, "block")?;
    let mut members = Map::new();
    for (id, list) in &block_tag_members {
        members.insert(
            id.clone(),
            Value::Array(list.iter().cloned().map(Value::String).collect()),
        );
    }
    put(
        &mut out,
        &mut coverage,
        "block_tag_members",
        Value::Object(members),
    );
    put(
        &mut out,
        &mut coverage,
        "block_tags",
        Value::Array(tags_of(&jar, "block")),
    );
    put(
        &mut out,
        &mut coverage,
        "item_tags",
        Value::Array(tags_of(&jar, "item")),
    );

    let (selected, available, enableable) = datapacks(&jar)?;
    put(&mut out, &mut coverage, "datapacks_selected", selected);
    put(&mut out, &mut coverage, "datapacks_available", available);
    put(&mut out, &mut coverage, "datapacks_enableable", enableable);
    put(
        &mut out,
        &mut coverage,
        "clock_time_markers",
        clock_time_markers(&jar)?,
    );

    put(
        &mut out,
        &mut coverage,
        "templates",
        Value::Array(jar.ids("data/minecraft/structure/", ".nbt")),
    );
    put(
        &mut out,
        &mut coverage,
        "enchantments",
        Value::Array(jar.ids("data/minecraft/enchantment/", ".json")),
    );
    put(
        &mut out,
        &mut coverage,
        "structures",
        Value::Array(jar.ids("data/minecraft/worldgen/structure/", ".json")),
    );
    put(
        &mut out,
        &mut coverage,
        "biomes",
        Value::Array(jar.ids("data/minecraft/worldgen/biome/", ".json")),
    );
    put(
        &mut out,
        &mut coverage,
        "damage_types",
        Value::Array(jar.ids("data/minecraft/damage_type/", ".json")),
    );
    put(
        &mut out,
        &mut coverage,
        "clocks",
        Value::Array(jar.ids("data/minecraft/world_clock/", ".json")),
    );
    put(
        &mut out,
        &mut coverage,
        "timelines",
        Value::Array(jar.ids("data/minecraft/timeline/", ".json")),
    );

    for (key, value) in source_fixtures() {
        put(&mut out, &mut coverage, key, value);
    }

    // ---- code-driven registries ----
    let mut code_registries: BTreeMap<&str, Vec<String>> = BTreeMap::new();

    let sounds = sound_events(&jar)?;
    coverage.note(&format!(
        "sounds: {} ids from SoundEvents.<clinit> plus the sound ids the sound-variant \
         and instrument data reference (vanilla builds 94 ids per variant in a loop)",
        sounds.len()
    ));
    code_registries.insert("minecraft:sound_event", sounds.clone());
    put(
        &mut out,
        &mut coverage,
        "sounds",
        Value::Array(sounds.into_iter().map(Value::String).collect()),
    );

    for (key, internal, registry) in [
        (
            "effects",
            "net/minecraft/world/effect/MobEffects",
            "minecraft:mob_effect",
        ),
        (
            "attributes",
            "net/minecraft/world/entity/ai/attributes/Attributes",
            "minecraft:attribute",
        ),
    ] {
        let class = jar.class(internal)?;
        let ids: Vec<String> = registered_ids(&class, "<clinit>")?.into_iter().collect();
        code_registries.insert(registry, ids.clone());
        put(
            &mut out,
            &mut coverage,
            key,
            Value::Array(ids.into_iter().map(Value::String).collect()),
        );
    }

    let entity = entity_types(&jar)?;
    code_registries.insert(
        "minecraft:entity_type",
        entity
            .iter()
            .map(|(id, _)| id.clone())
            .collect::<Vec<String>>(),
    );
    put(
        &mut out,
        &mut coverage,
        "entity_types",
        Value::Array(
            entity
                .iter()
                .map(|(id, _)| Value::String(id.clone()))
                .collect(),
        ),
    );
    put(
        &mut out,
        &mut coverage,
        "summonable",
        Value::Array(
            entity
                .iter()
                .filter(|(_, summonable)| *summonable)
                .map(|(id, _)| Value::String(id.clone()))
                .collect(),
        ),
    );

    let persistent = persistent_components(&jar)?;
    coverage.note(&format!(
        "persistent_components: {} components are persistent; the rest are transient, \
         told apart by the builder lambda the registration passes",
        persistent.len()
    ));
    put(
        &mut out,
        &mut coverage,
        "persistent_components",
        Value::Array(persistent.into_iter().map(Value::String).collect()),
    );

    // The item registry's ids are the item models the assets declare; its order in
    // `items` is registration order and is not in the assets.
    let mut items: Vec<String> = jar
        .entries("assets/minecraft/items/", ".json")
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    items.sort();
    code_registries.insert("minecraft:item", items.clone());

    if let Ok(value) = by_registry(&jar, &code_registries) {
        put(&mut out, &mut coverage, "by_registry", value);
        coverage.note(
            "by_registry: the data-driven registries plus item/entity_type/sound_event/\
             mob_effect/attribute; the other code-registered registries are not derivable \
             from the data pack",
        );
    }

    for key in ["items", "blocks", "block_entities"] {
        coverage.note(&format!(
            "{key}: not reproducible from client.jar alone (see the module docs)"
        ));
    }
    for (table, why) in BLOCKED_TABLES {
        coverage.note(&format!("{table}: {why}"));
    }

    Ok((Value::Object(out), coverage))
}

/// Every sound event id: the literals `SoundEvents.<clinit>` registers, plus the
/// ids the data-driven sound definitions reference.
///
/// Vanilla builds cat/chicken/cow/pig/wolf sound variants in a loop
/// (`"entity.cat." + name`), so those 94 ids are not string constants; they are,
/// however, named by the variant JSONs that use them.
fn sound_events(jar: &Jar) -> Result<Vec<String>, String> {
    let class = jar.class("net/minecraft/sounds/SoundEvents")?;
    let mut ids = registered_ids(&class, "<clinit>")?;
    for directory in [
        "cat_sound_variant",
        "chicken_sound_variant",
        "cow_sound_variant",
        "pig_sound_variant",
        "wolf_sound_variant",
        "instrument",
    ] {
        for (_, path) in jar.entries(&format!("data/minecraft/{directory}/"), ".json") {
            let document = jar.json(&path)?;
            collect_sound_refs(&document, &mut ids);
        }
    }
    Ok(ids.into_iter().collect())
}

/// Walk a data document for values that name a sound event.
fn collect_sound_refs(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (name, inner) in map {
                if let Value::String(text) = inner {
                    if name.ends_with("_sound") || name == "sound_event" || name == "sound" {
                        let id = if text.starts_with("minecraft:") {
                            text.clone()
                        } else if text.contains(':') {
                            text.clone()
                        } else {
                            format!("minecraft:{text}")
                        };
                        out.insert(id);
                    }
                } else {
                    collect_sound_refs(inner, out);
                }
            }
        }
        Value::Array(list) => {
            for item in list {
                collect_sound_refs(item, out);
            }
        }
        _ => {}
    }
}

/// The data components a saved item keeps: `DataComponents.<clinit>` registers each
/// one through a builder lambda, and a component is persistent when that lambda calls
/// `DataComponentType.Builder#persistent` (`isTransient` is its negation).
///
/// The lambda is found through the registration's bootstrap method, so this follows
/// the calls the class actually makes instead of guessing from the id.
fn persistent_components(jar: &Jar) -> Result<Vec<String>, String> {
    let class = jar.class("net/minecraft/core/component/DataComponents")?;
    let Some(code) = method_code(&class, "<clinit>") else {
        return Ok(Vec::new());
    };
    // Which of this class's own methods make a component persistent.
    let mut persistent_methods: BTreeSet<String> = BTreeSet::new();
    for member in &class.methods {
        let Some(body) = member.code.as_ref() else {
            continue;
        };
        let mut at = 0usize;
        while at < body.bytes.len() {
            if body.bytes[at] == 0xb6 {
                let index = u16_at(&body.bytes, at + 1)?;
                if let Some((_, name, _)) = classfile::method_ref(&class.constants, index) {
                    if name == "persistent" {
                        persistent_methods.insert(member.name.clone());
                    }
                }
            }
            at += instruction_len(&body.bytes, at)?;
        }
    }
    let mut ids = Vec::new();
    let mut name: Option<String> = None;
    let mut lambda: Option<usize> = None;
    let mut at = 0usize;
    while at < code.len() {
        match code[at] {
            0x12 => {
                name = classfile::string_constant(&class.constants, code[at + 1] as usize).or(name);
            }
            0x13 => {
                name = classfile::string_constant(&class.constants, u16_at(code, at + 1)?).or(name);
            }
            // invokedynamic: the builder lambda the registration is passed.
            0xba => lambda = Some(u16_at(code, at + 1)?),
            // invokestatic: `register(String, UnaryOperator)` closes the registration.
            0xb8 => {
                let index = u16_at(code, at + 1)?;
                if let Some((_, method, descriptor)) =
                    classfile::method_ref(&class.constants, index)
                {
                    if method == "register" && descriptor.starts_with("(Ljava/lang/String;") {
                        if let (Some(id), Some(lambda)) = (name.take(), lambda.take()) {
                            if lambda_is_persistent(&class, lambda, &persistent_methods) {
                                ids.push(format!("minecraft:{id}"));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        at += instruction_len(code, at)?;
    }
    ids.sort();
    Ok(ids)
}

fn lambda_is_persistent(
    class: &ClassFile,
    invokedynamic: usize,
    persistent_methods: &BTreeSet<String>,
) -> bool {
    let Some(classfile::Constant::InvokeDynamic { bootstrap, .. }) =
        class.constants.get(invokedynamic)
    else {
        return false;
    };
    let Some(row) = class.bootstraps.get(*bootstrap) else {
        return false;
    };
    row.arguments.iter().any(|argument| {
        let Some(classfile::Constant::MethodHandle { reference }) = class.constants.get(*argument)
        else {
            return false;
        };
        let Some((owner, name, _)) = classfile::method_ref(&class.constants, *reference) else {
            return false;
        };
        owner == class.name && persistent_methods.contains(&name)
    })
}

/// Every entity type, in registry order, with `EntityType#canSummon`.
///
/// `EntityTypes.<clinit>` registers one type per `EntityTypeIds.<NAME>` key; a
/// registration whose builder called `noSummon()` cannot be summoned (`player`
/// and `fishing_bobber` are the two).
fn entity_types(jar: &Jar) -> Result<Vec<(String, bool)>, String> {
    let ids = field_ids(
        &*jar.class("net/minecraft/world/entity/EntityTypeIds")?,
        "<clinit>",
    )?;
    let class = jar.class("net/minecraft/world/entity/EntityTypes")?;
    let mut out = Vec::new();
    for segment in segments(&class, "<clinit>")? {
        let Some((owner, field)) = segment.first_field else {
            continue;
        };
        if !owner.ends_with("EntityTypeIds") {
            continue;
        }
        let Some(id) = ids.get(&field) else {
            continue;
        };
        out.push((format!("minecraft:{id}"), !segment.no_summon));
    }
    Ok(out)
}

/// Write [`TABLE`] into `out_dir` and return what could not be produced.
pub fn write(jar_path: &Path, out_dir: &Path) -> Result<Coverage, String> {
    let (document, coverage) = command_registries(jar_path)?;
    fs::create_dir_all(out_dir).map_err(|error| format!("{}: {error}", out_dir.display()))?;
    let path = out_dir.join(TABLE);
    let text = serde_json::to_string_pretty(&document)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    fs::write(&path, text + "\n").map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(coverage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_learnable_types_exclude_the_dynamic_serializers() {
        assert!(!LEARNABLE_RECIPE_TYPES.contains(&"minecraft:crafting_special_repairitem"));
        assert!(!LEARNABLE_RECIPE_TYPES.contains(&"minecraft:brewing"));
        assert!(!LEARNABLE_RECIPE_TYPES.contains(&"minecraft:crafting_decorated_pot"));
        // A tipped arrow is crafted from a potion, but its recipe is learnable.
        assert!(LEARNABLE_RECIPE_TYPES.contains(&"minecraft:crafting_imbue"));
    }

    #[test]
    fn a_tag_reference_contributes_its_members_where_it_sits() {
        let mut files: BTreeMap<String, Vec<(String, bool)>> = BTreeMap::new();
        files.insert(
            "minecraft:base".into(),
            vec![("minecraft:a".into(), false), ("minecraft:b".into(), false)],
        );
        files.insert(
            "minecraft:wide".into(),
            vec![
                ("minecraft:c".into(), false),
                ("minecraft:base".into(), true),
                // A duplicate of a member the reference already brought in.
                ("minecraft:a".into(), true),
            ],
        );
        let tags = {
            let mut resolved = BTreeMap::new();
            for id in files.keys() {
                let mut visiting = BTreeSet::new();
                let members = resolve_tag(id, &files, &mut resolved, &mut visiting).unwrap();
                resolved.insert(id.clone(), members);
            }
            resolved
        };
        assert_eq!(
            tags["minecraft:wide"],
            vec!["minecraft:c", "minecraft:a", "minecraft:b"]
        );
    }

    #[test]
    fn a_tag_that_includes_itself_is_an_error() {
        let mut files: BTreeMap<String, Vec<(String, bool)>> = BTreeMap::new();
        files.insert(
            "minecraft:loop".into(),
            vec![("minecraft:loop".into(), true)],
        );
        let mut resolved = BTreeMap::new();
        let mut visiting = BTreeSet::new();
        assert!(resolve_tag("minecraft:loop", &files, &mut resolved, &mut visiting).is_err());
    }

    #[test]
    fn segments_split_at_putstatic_and_keep_the_register_flag() {
        // Hand-assembled: ldc "id"; invokestatic register(String); putstatic A;
        // then ldc "other"; putstatic A.
        use classfile::Constant;
        let constants = vec![
            Constant::Unused,
            Constant::Utf8("id".into()),
            Constant::String("#1".into()),
            Constant::Utf8("other".into()),
            Constant::String("#3".into()),
            Constant::Utf8("register".into()),
            Constant::Utf8("(Ljava/lang/String;)V".into()),
            Constant::NameAndType {
                name: "#5".into(),
                kind: "#6".into(),
            },
            Constant::Class("#9".into()),
            Constant::Utf8("net/minecraft/core/Registry".into()),
            Constant::Methodref {
                class: "#8".into(),
                name: "#7".into(),
                kind: "M".into(),
            },
            Constant::Class("#12".into()),
            Constant::Utf8("net/minecraft/world/item/Items".into()),
            Constant::Utf8("A".into()),
            Constant::Utf8("Lnet/minecraft/world/item/Item;".into()),
            Constant::NameAndType {
                name: "#13".into(),
                kind: "#14".into(),
            },
            Constant::Fieldref {
                class: "#11".into(),
                name: "#15".into(),
                kind: "F".into(),
            },
        ];
        let code = vec![
            0x12, 2, // ldc #2 ("id")
            0xb8, 0x00, 10, // invokestatic #10
            0xb3, 0x00, 16, // putstatic #16
            0x12, 4, // ldc #4 ("other")
            0xb3, 0x00, 16, // putstatic #16
        ];
        let class = ClassFile {
            name: "test".into(),
            super_name: String::new(),
            constants,
            fields: Vec::new(),
            methods: vec![classfile::Member {
                access: 8,
                name: "<clinit>".into(),
                descriptor: "()V".into(),
                code: Some(classfile::Code {
                    max_stack: 2,
                    max_locals: 0,
                    bytes: code,
                }),
            }],
            bootstraps: Vec::new(),
        };
        let segments = segments(&class, "<clinit>").unwrap();
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].first_string.as_deref(), Some("id"));
        assert!(segments[0].registers);
        assert_eq!(
            segments[0].field.as_ref().map(|(_, name)| name.as_str()),
            Some("A")
        );
        assert_eq!(segments[1].first_string.as_deref(), Some("other"));
        assert!(!segments[1].registers);
    }
}
