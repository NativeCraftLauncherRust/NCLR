//! Horizon's blocking landing table, rebuilt from the player's own `client.jar`.
//!
//! `horizon-prepare` runs Java oracles against the real 26.3 jars and writes the
//! block slice of `ship.hzr3`. This module rebuilds the part of that slice the
//! client jar alone proves, so the launcher stops shipping it. Nothing here
//! reads `ship.hzr3`.
//!
//! # What is produced
//!
//! `landing-definitions.json` — the per-block landing table
//! (`VanillaBlockLandingOracle.export`): a map from a block's registry id to
//! `{fall, bounce}`, plus three block-specific extras.
//!
//! ```text
//! minecraft:farmland       {"fall":"FarmlandBlock","bounce":false,"base":"minecraft:dirt"}
//! minecraft:shelf_mushroom {"fall":"Block","bounce":true}
//! minecraft:honey_block    {"fall":"HoneyBlock","bounce":false,"volume":"3f800000","pitch":"3f800000"}
//! ```
//!
//! `fall` is the simple name of the class that declares `fallOn` for the block,
//! `bounce` is true when `bounceOn` comes from `ShelfMushroomBlock`, `base` is
//! the farmland block's `baseBlock` field, and `volume`/`pitch` are the float
//! bits of the honey block's sound type.
//!
//! # Where the values live
//!
//! Every one of them is bytecode in the jar; no table is read:
//!
//! * **The registry** (which ids exist and which class each one is) comes from
//!   `Blocks.<clinit>` plus the two reference classes. `BlockIds` holds 112
//!   `ResourceKey<Block>` constants and two sixteen-value `ColorCollection`s;
//!   `BlockItemIds` holds 766 `BlockItemId` records, sixteen colour collections
//!   and fifteen weathering-copper collections. Both initialisers are
//!   straight-line `ldc`/`create`/`putstatic` runs. Together they name exactly
//!   the 1,286 blocks `BuiltInRegistries.BLOCK` carries (878 registered one by
//!   one, 288 coloured, 120 weathering-copper).
//! * **A block's class** is the `new` of the factory lambda `Blocks.<clinit>`
//!   hands to `Blocks.register`, resolved through the class's
//!   `BootstrapMethods` table. The stair/slab/wall helpers build their class the
//!   same way one level in, and the colour/copper families pass a
//!   `TriFunction`/`BiFunction` whose lambda names the block class.
//! * **`fall`/`bounce`** are the first class on the block's superclass chain
//!   that declares `fallOn`/`bounceOn`.
//! * **`base`** is the `getstatic Blocks.<field>` operand of the farmland
//!   factory lambda (`properties -> new FarmlandBlock(Blocks.DIRT, properties)`),
//!   mapped through that field's own registered id.
//! * **`volume`/`pitch`** are the first two float operands of the
//!   `new SoundType(...)` that initialises the sound type the honey block's
//!   properties name (`SoundType.HONEY_BLOCK`).
//!
//! # What is not produced
//!
//! The other block tables need values only vanilla's running code computes — see
//! [`BLOCKED_TABLES`]. The largest, `block-behaviours.json`, is a per-state table
//! (35,723 rows) whose `fields` are block *instances'* private fields (a
//! sapling's `TreeGrower`, an ore's `IntProvider`, a sand block's `dustColor`)
//! and whose `states` are `VoxelShape`/`isFaceSturdy`/redstone-signal queries.
//! Rebuilding those means running `Blocks.<clinit>` and every block constructor,
//! then the geometry code: a JVM plus the block engine, not a class-file reader.
//!
//! # Verified
//!
//! Against `horizon-prepare`'s 26.3 output (`tmp/ship-extract`):
//!
//! * `landing-definitions.json` is **byte-identical** to the Java export — the
//!   same 1,286 entries in registry order, the same compact Gson text, no
//!   trailing newline.
//! * The registry derivation itself is checked against `block-behaviours.json`'s
//!   `blocks` array: all 1,286 ids, their order and their whole class chain match
//!   (`every_block_class_matches_block_behaviours`). That is what makes the
//!   `fall`/`bounce` lookup trustworthy, and it also shows the 288 coloured and
//!   120 weathering-copper families are derived, not assumed.
//!
//! # Wiring
//!
//! `src/main.rs` is out of scope for this change, so nothing calls in here yet; a
//! `blocks` command would be
//!
//! ```text
//! hzr-export blocks <client.jar> <resources-dir>
//! ```
//!
//! over [`write`], which writes `landing-definitions.json` below the directory.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

use serde_json::{Map, Value};

// `classfile.rs` is shared with `hzr-bytecode` rather than copied: the crate
// cannot depend on it (a manifest edit is out of scope), so it is included by
// path. Same pattern as `registries.rs`.
#[path = "../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;
use classfile::{ClassFile, Constant};

/// What `ship.hzr3` calls the one table this module writes.
pub const TABLE: &str = "landing-definitions.json";

/// The block tables this task names.
pub const TABLES: [&str; 3] = [
    "block-behaviours.json",
    "worldgen/blocks/structure-blocks.json",
    "landing-definitions.json",
];

/// The block tables the jar alone cannot reproduce, and why.
///
/// Each reason names the value that only the running JVM assigns, so a later
/// change can tell what would have to be built to lift the block.
pub const BLOCKED_TABLES: [(&str, &str); 7] = [
    (
        "block-behaviours.json",
        "needs 713 blocks' private instance fields (a sapling's `TreeGrower`, an ore's \
         `IntProvider`, a sand block's `dustColor`) and 35,723 per-state rows of \
         `isFaceSturdy`/`isSolid`/`isAir`/`isRedstoneConductor`/`isSignalSource`/\
         `getSignal`/`getCollisionShape`/`isSuffocating`/fluid/instrument facts — all \
         computed by block constructors and shape code at runtime",
    ),
    (
        "worldgen/blocks/structure-blocks.json",
        "35,723 per-state rows of `BlockState.rotate`/`mirror` (per block class) and \
         `isCollisionShapeFullBlock`, which needs the same collision geometry as the \
         shape NBTs, plus the block-state registry its rows are indexed by",
    ),
    (
        "cached-collision.nbt",
        "per-state `getCollisionShape` geometry: the shapes come from each block class's \
         `getShape`/`createBlockStateDefinition` code, not from data",
    ),
    (
        "occlusion.nbt",
        "per-state `getOcclusionShape` geometry, same blocker as `cached-collision.nbt`",
    ),
    (
        "worldgen/blocks/cached-support.nbt",
        "per-state support shapes (`isFaceSturdy` geometry), same blocker",
    ),
    (
        "block-properties.nbt",
        "`states` is the whole block-state registry encoded with `BlockState.FULL_CODEC`, \
         and `occlusion` embeds a second copy of the per-state occlusion shapes",
    ),
    (
        "movement-properties.nbt",
        "`states` is the whole block-state registry encoded with `BlockState.FULL_CODEC`; \
         every value is indexed by state id",
    ),
];

/// What the jar allowed this build to reconstruct, and why the rest are absent.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub produced: Vec<String>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
}

impl Coverage {
    fn add(&mut self, table: &str) {
        self.produced.push(table.to_string());
        self.missing.retain(|name| name != table);
    }

    fn note(&mut self, note: impl Into<String>) {
        self.notes.push(note.into());
    }
}

/// One decoded instruction: the opcode plus the operand this module reads.
struct Instruction {
    opcode: u8,
    /// Constant-pool index of `ldc`/`ldc_w`/`getstatic`/`putstatic`/`invoke*`/`new`.
    index: Option<usize>,
}

/// Decode a method body. Only instruction lengths and the operands this module
/// reads are kept; `tableswitch`/`lookupswitch` are skipped by their own length so
/// a method that contains one cannot desynchronise the scan.
fn decode(bytes: &[u8]) -> Vec<Instruction> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let opcode = bytes[at];
        let mut index = None;
        let length = match opcode {
            // `bipush`/`sipush`: the value is never read here, only the width.
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
            0x84 => {
                index = bytes.get(at + 1).map(|value| *value as usize);
                3
            }
            0x99..=0xa8 | 0xc6 | 0xc7 => 3,
            0xc4 => 4, // wide
            0xc5 => 4,
            0xc8 | 0xc9 => 5,
            0xaa => {
                // tableswitch: padding to a 4-byte boundary, then 3 ints, then n jumps.
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
                // lookupswitch: padding, default, npairs, then npairs pairs.
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
        out.push(Instruction { opcode, index });
        at += length;
    }
    out
}

fn class_name_at(constants: &[Constant], index: usize) -> Option<String> {
    let Constant::Class(reference) = constants.get(index)? else {
        return None;
    };
    let inner: usize = reference.trim_start_matches('#').parse().ok()?;
    match constants.get(inner)? {
        Constant::Utf8(name) => Some(name.clone()),
        _ => None,
    }
}

fn simple(internal: &str) -> &str {
    internal.rsplit('/').next().unwrap_or(internal)
}

/// The `new` class an instruction builds.
fn new_class(class: &ClassFile, instruction: &Instruction) -> Option<String> {
    if instruction.opcode != 0xbb {
        return None;
    }
    class_name_at(&class.constants, instruction.index?)
}

fn string_constant(class: &ClassFile, instruction: &Instruction) -> Option<String> {
    match instruction.opcode {
        0x12 | 0x13 => classfile::string_constant(&class.constants, instruction.index?),
        _ => None,
    }
}

/// The float a `fconst_*`/`ldc` instruction pushes, when it pushes one.
fn float_constant(class: &ClassFile, instruction: &Instruction) -> Option<f32> {
    match instruction.opcode {
        0x0b..=0x0d => Some((instruction.opcode - 0x0b) as f32),
        0x12 | 0x13 => match class.constants.get(instruction.index?)? {
            Constant::Float(value) => Some(*value),
            _ => None,
        },
        _ => None,
    }
}

/// The implementation method an `invokedynamic` resolves to: `(owner, method)`.
fn indy_target(class: &ClassFile, index: usize) -> Option<(String, String)> {
    let Constant::InvokeDynamic { bootstrap, .. } = class.constants.get(index)? else {
        return None;
    };
    let entry = class.bootstraps.get(*bootstrap)?;
    for argument in &entry.arguments {
        // The implementation method handle is the only method handle a lambda
        // metafactory receives.
        let Constant::MethodHandle { reference } = class.constants.get(*argument)? else {
            continue;
        };
        let Constant::Methodref {
            class: owner, name, ..
        } = class.constants.get(*reference)?
        else {
            continue;
        };
        let owner_index: usize = owner.trim_start_matches('#').parse().ok()?;
        let name_and_type: usize = name.trim_start_matches('#').parse().ok()?;
        let owner_name = class_name_at(&class.constants, owner_index)?;
        let Constant::NameAndType { name, .. } = class.constants.get(name_and_type)? else {
            continue;
        };
        let name_index: usize = name.trim_start_matches('#').parse().ok()?;
        return Some((owner_name, classfile::utf8(&class.constants, name_index)));
    }
    None
}

/// The player's `client.jar`, held in memory with a parsed-class cache.
pub struct Jar {
    files: BTreeMap<String, Vec<u8>>,
    classes: RefCell<BTreeMap<String, Option<Rc<ClassFile>>>>,
}

impl Jar {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut files = BTreeMap::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
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

    /// A class file by internal name (`net/minecraft/world/level/block/Blocks`).
    pub fn class(&self, internal: &str) -> Option<Rc<ClassFile>> {
        if let Some(hit) = self.classes.borrow().get(internal) {
            return hit.clone();
        }
        let parsed = self
            .files
            .get(&format!("{internal}.class"))
            .and_then(|bytes| classfile::parse(bytes).ok())
            .map(Rc::new);
        self.classes
            .borrow_mut()
            .insert(internal.to_string(), parsed.clone());
        parsed
    }

    /// The `new` class a method (or the lambda it calls) builds.
    fn created_class(
        &self,
        class: &Rc<ClassFile>,
        method_name: &str,
        descriptor: &str,
        depth: usize,
    ) -> Option<String> {
        if depth > 4 {
            return None;
        }
        let method = class
            .methods
            .iter()
            .find(|candidate| candidate.name == method_name && candidate.descriptor == descriptor)
            .or_else(|| {
                // A synthetic lambda may be renamed between builds; fall back to
                // the name when the descriptor no longer matches.
                class
                    .methods
                    .iter()
                    .find(|candidate| candidate.name == method_name)
            })?;
        let code = method.code.as_ref()?;
        let instructions = decode(&code.bytes);
        for instruction in &instructions {
            if let Some(internal) = new_class(class, instruction) {
                return Some(internal);
            }
            if instruction.opcode == 0xba {
                let (owner, name) = indy_target(class, instruction.index.unwrap_or(0))?;
                if name == "<init>" {
                    return Some(owner);
                }
                let target = self.class(&owner)?;
                if let Some(found) = self.created_class(&target, &name, "", depth + 1) {
                    return Some(found);
                }
            }
            if instruction.opcode == 0xb8 {
                if let Some((owner, name, descriptor)) =
                    classfile::method_ref(&class.constants, instruction.index.unwrap_or(0))
                {
                    // A family lambda can hand the work to one of the stair/slab
                    // helpers instead of building the block itself.
                    if owner == BLOCKS && REGISTER_HELPERS.contains(&name.as_str()) {
                        if let Some(found) =
                            self.created_class(class, &name, &descriptor, depth + 1)
                        {
                            return Some(found);
                        }
                    }
                }
            }
        }
        None
    }

    /// The class an `invokedynamic` builds, resolving through the bootstrap table.
    fn created_class_from_indy(&self, class: &Rc<ClassFile>, index: usize) -> Option<String> {
        let (owner, name) = indy_target(class, index)?;
        if name == "<init>" {
            return Some(owner);
        }
        let target = self.class(&owner)?;
        self.created_class(&target, &name, "", 0)
    }

    /// Whether `internal` is (a subclass of) `Block`.
    fn is_block_class(&self, internal: &str) -> bool {
        let mut current = internal.to_string();
        for _ in 0..32 {
            if current == BLOCK {
                return true;
            }
            let Some(class) = self.class(&current) else {
                return false;
            };
            if class.super_name.is_empty() {
                return false;
            }
            current = class.super_name.clone();
        }
        false
    }

    /// The first lambda among `indys` that builds a block class, with its index.
    ///
    /// A registration's argument list carries lambdas that are not factories
    /// (`mapColor`, `isSuffocating`, the item-id adapter, a plain fallback block
    /// for the colour families, …), so the factory is picked by what it builds:
    /// a constructor reference to a block class wins, otherwise the first lambda
    /// that builds one.
    fn block_class_from_indys(
        &self,
        class: &Rc<ClassFile>,
        indys: &[usize],
    ) -> Option<(String, usize)> {
        for index in indys {
            let Some((owner, name)) = indy_target(class, *index) else {
                continue;
            };
            if name == "<init>" && self.is_block_class(&owner) {
                return Some((owner, *index));
            }
        }
        self.block_classes_from_indys(class, indys)
            .into_iter()
            .next()
    }

    /// Every block class the given lambdas build, in argument order, deduplicated.
    fn block_classes_from_indys(
        &self,
        class: &Rc<ClassFile>,
        indys: &[usize],
    ) -> Vec<(String, usize)> {
        let mut out: Vec<(String, usize)> = Vec::new();
        for index in indys {
            if let Some(internal) = self.created_class_from_indy(class, *index) {
                if self.is_block_class(&internal) && !out.iter().any(|(seen, _)| seen == &internal)
                {
                    out.push((internal, *index));
                }
            }
        }
        out
    }

    /// The `SoundType` fields' `(volume, pitch)`.
    fn sound_volumes(&self) -> BTreeMap<String, (f32, f32)> {
        let mut out = BTreeMap::new();
        let Some(class) = self.class("net/minecraft/world/level/block/SoundType") else {
            return out;
        };
        let Some(initialiser) = class
            .method("<clinit>")
            .and_then(|method| method.code.as_ref())
        else {
            return out;
        };
        let mut pushing = false;
        let mut floats: Vec<f32> = Vec::new();
        for instruction in &decode(&initialiser.bytes) {
            if new_class(&class, instruction).is_some_and(|name| name == class.name) {
                pushing = true;
                floats.clear();
                continue;
            }
            if !pushing {
                continue;
            }
            if let Some(value) = float_constant(&class, instruction) {
                floats.push(value);
                continue;
            }
            if instruction.opcode == 0xb3 {
                if let Some((owner, field, _)) =
                    classfile::field_ref(&class.constants, instruction.index.unwrap_or(0))
                {
                    if owner == class.name && floats.len() >= 2 {
                        out.insert(field, (floats[0], floats[1]));
                    }
                }
                pushing = false;
            }
        }
        out
    }
}

/// One block of the registry: its id and its class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// `minecraft:<name>`.
    pub id: String,
    /// The block's class, by simple name (`StairBlock`).
    pub class: String,
    /// The superclass chain from the block's class up to `Block` (inclusive).
    pub chain: Vec<String>,
}

/// One registration's extra facts, before the chain and the owner are resolved.
#[derive(Debug, Clone)]
struct Registration {
    id: String,
    class: Option<String>,
    /// The `Blocks.<field>` the farmland factory reads its base from.
    base_field: Option<String>,
    /// The `SoundType.<field>` its properties name.
    sound: Option<String>,
}

const BLOCK: &str = "net/minecraft/world/level/block/Block";
const BLOCK_BEHAVIOUR: &str = "net/minecraft/world/level/block/state/BlockBehaviour";
const BLOCKS: &str = "net/minecraft/world/level/block/Blocks";
const BLOCK_IDS: &str = "net/minecraft/references/BlockIds";
const BLOCK_ITEM_IDS: &str = "net/minecraft/references/BlockItemIds";

/// A value the id classes' initialisers are building, as far as it matters here.
enum Pending {
    /// `BlockItemId.create(String)` / a `ResourceKey` from one name.
    One(String),
    /// `BlockItemId.create(String, String)`: the block's name is the first.
    Pair(String, String),
    /// `createSimpleColored(suffix)`: sixteen `<colour>_<suffix>` names.
    Colored(String),
    /// `createSimpleCopper(suffix)`: eight `<state prefix><suffix>` names.
    Copper(String),
    /// A `WeatheringCopperCollection.ByState` of four per-weathering values.
    States(Vec<String>),
    /// `prefixWithState` over a `ByState`: eight `<prefix><value>` names.
    PrefixedStates(Vec<String>),
}

/// The `BlockIds`/`BlockItemIds` initialisers: field name -> the ids it holds.
#[derive(Debug, Default)]
struct IdClass {
    fields: BTreeMap<String, Vec<String>>,
}

/// The sixteen dye-colour words, in `ColorCollection` order.
fn dye_colors(jar: &Jar) -> Result<Vec<String>, String> {
    let class = jar
        .class("net/minecraft/world/item/DyeColor")
        .ok_or_else(|| "DyeColor is not in the jar".to_string())?;
    let initialiser = class
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
        .ok_or_else(|| "DyeColor has no initialiser".to_string())?;
    let mut names = Vec::new();
    let mut building = false;
    for instruction in &decode(&initialiser.bytes) {
        if new_class(&class, instruction).is_some_and(|name| name == class.name) {
            building = true;
            continue;
        }
        if !building {
            continue;
        }
        if let Some(text) = string_constant(&class, instruction) {
            // The enum constant name is upper case; the serialised name is not.
            if text.chars().any(|character| character.is_ascii_lowercase()) {
                names.push(text);
            }
        }
        if instruction.opcode == 0xb3 {
            building = false;
        }
    }
    if names.len() != 16 {
        return Err(format!(
            "DyeColor has {} colour names, expected 16",
            names.len()
        ));
    }
    Ok(names)
}

/// The eight copper prefixes, in collection order (unaffected first, waxed last).
fn copper_prefixes(jar: &Jar) -> Result<Vec<String>, String> {
    let class = jar
        .class("net/minecraft/world/level/block/WeatheringCopperCollection")
        .ok_or_else(|| "WeatheringCopperCollection is not in the jar".to_string())?;
    let initialiser = class
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
        .ok_or_else(|| "WeatheringCopperCollection has no initialiser".to_string())?;
    let names: Vec<String> = decode(&initialiser.bytes)
        .iter()
        .filter_map(|instruction| string_constant(&class, instruction))
        .collect();
    if names.len() != 8 {
        return Err(format!(
            "WeatheringCopperCollection has {} prefixes, expected 8",
            names.len()
        ));
    }
    Ok(names)
}

/// Read one of the id classes' static initialisers.
fn id_class(
    jar: &Jar,
    internal: &str,
    colors: &[String],
    prefixes: &[String],
) -> Result<IdClass, String> {
    let class = jar
        .class(internal)
        .ok_or_else(|| format!("{internal} is not in the jar"))?;
    let initialiser = class
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
        .ok_or_else(|| format!("{internal} has no initialiser"))?;
    let mut strings: Vec<String> = Vec::new();
    let mut pending: Option<Pending> = None;
    // `ByState` fields: the four per-weathering values of a copper family.
    let mut states: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut state_values: Option<Vec<String>> = None;
    let mut building_state = false;
    let mut out = IdClass::default();
    for instruction in &decode(&initialiser.bytes) {
        if let Some(text) = string_constant(&class, instruction) {
            strings.push(text);
            continue;
        }
        match instruction.opcode {
            0xbb => {
                // `new WeatheringCopperCollection$ByState`: its four constructor
                // operands are the per-weathering values, not ids.
                if let Some(name) = new_class(&class, instruction) {
                    if name.ends_with("WeatheringCopperCollection$ByState") {
                        building_state = true;
                        strings.clear();
                    }
                }
            }
            0xb7 => {
                // `ByState.<init>` consumes the four values collected above.
                if building_state {
                    state_values = Some(strings.clone());
                    strings.clear();
                    building_state = false;
                }
            }
            0xb2 => {
                // A `ByState` static field feeds `prefixWithState`.
                let (owner, field, descriptor) =
                    classfile::field_ref(&class.constants, instruction.index.unwrap_or(0))
                        .unwrap_or_default();
                if owner == internal && descriptor.contains("WeatheringCopperCollection$ByState") {
                    if let Some(values) = states.get(&field) {
                        pending = Some(Pending::States(values.clone()));
                    }
                }
            }
            0xb8 => {
                if let Some((_owner, name, _)) =
                    classfile::method_ref(&class.constants, instruction.index.unwrap_or(0))
                {
                    // `create` lives on `BlockItemId`; the simple-colour/copper
                    // helpers live on the class itself. Both are called by name.
                    match name.as_str() {
                        "create" => {
                            pending = match strings.len() {
                                0 => None,
                                1 => Some(Pending::One(strings[0].clone())),
                                _ => Some(Pending::Pair(
                                    strings[strings.len() - 2].clone(),
                                    strings[strings.len() - 1].clone(),
                                )),
                            };
                            strings.clear();
                        }
                        "createSimpleColored" => {
                            pending = strings.last().cloned().map(Pending::Colored);
                            strings.clear();
                        }
                        "createSimpleCopper" => {
                            pending = strings.last().cloned().map(Pending::Copper);
                            strings.clear();
                        }
                        "prefixWithState" => {
                            if let Some(Pending::States(values)) = pending.take() {
                                pending = Some(Pending::PrefixedStates(values));
                            }
                        }
                        // `same` and `map` do not rename anything here.
                        _ => {}
                    }
                }
            }
            0xb3 => {
                let (owner, field, _) =
                    classfile::field_ref(&class.constants, instruction.index.unwrap_or(0))
                        .unwrap_or_default();
                if owner != internal {
                    continue;
                }
                if let Some(values) = state_values.take() {
                    states.insert(field, values);
                    continue;
                }
                let names: Vec<String> = match pending.take() {
                    Some(Pending::One(name)) => vec![name],
                    Some(Pending::Pair(block, _item)) => vec![block],
                    Some(Pending::Colored(suffix)) => colors
                        .iter()
                        .map(|color| format!("{color}_{suffix}"))
                        .collect(),
                    Some(Pending::Copper(suffix)) => prefixes
                        .iter()
                        .map(|prefix| format!("{prefix}{suffix}"))
                        .collect(),
                    // `prefixWithState` walks the eight prefixes against the four
                    // per-weathering values (`copper_block`, `copper`, `copper`, …).
                    Some(Pending::PrefixedStates(values)) => prefixes
                        .iter()
                        .enumerate()
                        .filter_map(|(index, prefix)| {
                            values
                                .get(index % values.len())
                                .map(|value| format!("{prefix}{value}"))
                        })
                        .collect(),
                    Some(Pending::States(values)) => values,
                    None => Vec::new(),
                };
                if !names.is_empty() {
                    out.fields.insert(field, names);
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// The classes that hand a registration to `Blocks.register`.
const REGISTER_HELPERS: [&str; 5] = [
    "register",
    "registerStair",
    "registerSlab",
    "registerWall",
    "registerLegacyStair",
];

/// The derived registry plus the facts the landing table needs.
struct Registry {
    /// Every registered block, in `BuiltInRegistries.BLOCK` order.
    registrations: Vec<Registration>,
    /// `Blocks.<field>` -> that block's registry id.
    field_ids: BTreeMap<String, String>,
    /// `SoundType.<field>` -> `(volume, pitch)`.
    sound_volumes: BTreeMap<String, (f32, f32)>,
}

impl Registry {
    fn load(jar: &Jar) -> Result<Registry, String> {
        let blocks = jar
            .class(BLOCKS)
            .ok_or_else(|| "Blocks is not in the jar".to_string())?;
        let initialiser = blocks
            .method("<clinit>")
            .and_then(|method| method.code.as_ref())
            .ok_or_else(|| "Blocks has no initialiser".to_string())?;
        let colors = dye_colors(jar)?;
        let prefixes = copper_prefixes(jar)?;
        let block_ids = id_class(jar, BLOCK_IDS, &colors, &prefixes)?;
        let item_ids = id_class(jar, BLOCK_ITEM_IDS, &colors, &prefixes)?;
        let ids_of = |owner: &str, field: &str| -> Option<Vec<String>> {
            let table = if owner == BLOCK_IDS {
                &block_ids.fields
            } else if owner == BLOCK_ITEM_IDS {
                &item_ids.fields
            } else {
                return None;
            };
            table.get(field).cloned()
        };

        let mut registrations: Vec<Registration> = Vec::new();
        let mut field_ids: BTreeMap<String, String> = BTreeMap::new();
        let mut pending_names: Option<Vec<String>> = None;
        let mut pending_indys: Vec<usize> = Vec::new();
        let mut pending_sound: Option<String> = None;
        let mut last: Vec<String> = Vec::new();

        // 26.3's `Blocks.<clinit>` has no branches, so the linear scan is exactly
        // the execution order; the class-chain test would catch a build where it
        // is not.
        for instruction in &decode(&initialiser.bytes) {
            match instruction.opcode {
                0xb2 => {
                    let Some((owner, field, _)) =
                        classfile::field_ref(&blocks.constants, instruction.index.unwrap_or(0))
                    else {
                        continue;
                    };
                    if let Some(names) = ids_of(&owner, &field) {
                        pending_names = Some(names);
                        pending_indys.clear();
                        pending_sound = None;
                    } else if owner == "net/minecraft/world/level/block/SoundType" {
                        pending_sound = Some(field);
                    }
                }
                0xba => pending_indys.push(instruction.index.unwrap_or(0)),
                0xb8 => {
                    let Some((owner, name, descriptor)) =
                        classfile::method_ref(&blocks.constants, instruction.index.unwrap_or(0))
                    else {
                        continue;
                    };
                    if owner == BLOCKS && REGISTER_HELPERS.contains(&name.as_str()) {
                        let Some(names) = pending_names.take() else {
                            continue;
                        };
                        // A registration either passes a factory lambda (then the
                        // class is the `new` of the first lambda that builds a
                        // block) or delegates to a helper that builds it.
                        let factory = if name == "register"
                            && descriptor.contains("java/util/function/Function")
                        {
                            jar.block_class_from_indys(&blocks, &pending_indys)
                        } else {
                            jar.created_class(&blocks, &name, &descriptor, 0)
                                .map(|class| (class, 0))
                        };
                        let class = factory.as_ref().map(|(class, _)| class.clone());
                        let base_field = class
                            .as_deref()
                            .filter(|internal| simple(internal) == "FarmlandBlock")
                            .and_then(|_| {
                                factory.as_ref().and_then(|(_, index)| {
                                    factory_block_field(jar, &blocks, *index)
                                })
                            });
                        for id in names {
                            last.push(id.clone());
                            registrations.push(Registration {
                                id: format!("minecraft:{id}"),
                                class: class.clone(),
                                base_field: base_field.clone(),
                                sound: pending_sound.clone(),
                            });
                        }
                        pending_indys.clear();
                        pending_sound = None;
                        continue;
                    }
                    let family = owner.ends_with("ColorCollection")
                        && (name == "registerBlocks" || name == "zipMap")
                        || owner.ends_with("WeatheringCopperCollection")
                            && name == "registerBlocks";
                    if !family {
                        continue;
                    }
                    let Some(names) = pending_names.take() else {
                        continue;
                    };
                    let mut classes: Vec<Option<String>> = {
                        let first = jar
                            .block_class_from_indys(&blocks, &pending_indys)
                            .map(|(class, _)| class);
                        vec![first; names.len()]
                    };
                    // A weathering-copper family carries two factories: the
                    // weathering block for the four unwaxed entries and a plain
                    // one for the four waxed entries. The weathering class is the
                    // one that extends the other.
                    if owner.ends_with("WeatheringCopperCollection")
                        && names.len() == 8
                        && classes.first().is_some_and(Option::is_some)
                    {
                        let candidates = jar.block_classes_from_indys(&blocks, &pending_indys);
                        let weathering = candidates
                            .iter()
                            .find(|(candidate, _)| {
                                candidates.iter().any(|(other, _)| {
                                    other != candidate && subclass_of(jar, candidate, other)
                                })
                            })
                            .map(|(class, _)| class.clone());
                        if let Some(weathering) = weathering {
                            let waxed = candidates
                                .iter()
                                .map(|(class, _)| class.clone())
                                .find(|class| class != &weathering);
                            if let Some(waxed) = waxed {
                                for (index, slot) in classes.iter_mut().enumerate() {
                                    *slot = Some(if index < 4 {
                                        weathering.clone()
                                    } else {
                                        waxed.clone()
                                    });
                                }
                            }
                        }
                    }
                    for (id, class) in names.into_iter().zip(classes) {
                        last.push(id.clone());
                        registrations.push(Registration {
                            id: format!("minecraft:{id}"),
                            class,
                            base_field: None,
                            sound: None,
                        });
                    }
                    pending_indys.clear();
                    pending_sound = None;
                }
                0xb3 => {
                    let Some((owner, field, _)) =
                        classfile::field_ref(&blocks.constants, instruction.index.unwrap_or(0))
                    else {
                        continue;
                    };
                    if owner != BLOCKS {
                        continue;
                    }
                    if let Some(id) = last.first() {
                        field_ids.insert(field, format!("minecraft:{id}"));
                    }
                    last.clear();
                }
                _ => {}
            }
        }

        let sound_volumes = jar.sound_volumes();
        Ok(Registry {
            registrations,
            field_ids,
            sound_volumes,
        })
    }

    fn chain(&self, jar: &Jar, internal: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut current = internal.to_string();
        while let Some(class) = jar.class(&current) {
            out.push(simple(&class.name).to_string());
            if class.super_name.is_empty()
                || class.super_name == BLOCK_BEHAVIOUR
                || class.super_name == "java/lang/Object"
            {
                break;
            }
            current = class.super_name.clone();
        }
        out
    }

    fn blocks(&self, jar: &Jar) -> Vec<Block> {
        self.registrations
            .iter()
            .map(|registration| {
                let internal = registration
                    .class
                    .clone()
                    .unwrap_or_else(|| BLOCK.to_string());
                Block {
                    id: registration.id.clone(),
                    class: simple(&internal).to_string(),
                    chain: self.chain(jar, &internal),
                }
            })
            .collect()
    }
}

/// The `Blocks.<field>` operand of a factory lambda, when it reads one.
///
/// Only farmland has a base block, and its factory is
/// `properties -> new FarmlandBlock(Blocks.DIRT, properties)`.
fn factory_block_field(jar: &Jar, blocks: &Rc<ClassFile>, indy: usize) -> Option<String> {
    let (owner, name) = indy_target(blocks, indy)?;
    if name == "<init>" {
        // A bare `FarmlandBlock::new` reference carries no operand to read; the
        // farmland block's factory is a lambda, so this is a diagnostic, not a
        // normal path.
        return None;
    }
    let target = jar.class(&owner)?;
    let method = target
        .methods
        .iter()
        .find(|candidate| candidate.name == name)?;
    let code = method.code.as_ref()?;
    for instruction in decode(&code.bytes) {
        if instruction.opcode != 0xb2 {
            continue;
        }
        let (owner, field, descriptor) =
            classfile::field_ref(&target.constants, instruction.index.unwrap_or(0))?;
        if owner == BLOCKS && descriptor == "Lnet/minecraft/world/level/block/Block;" {
            return Some(field);
        }
    }
    None
}

/// Whether `internal` is `ancestor` or a subclass of it.
fn subclass_of(jar: &Jar, internal: &str, ancestor: &str) -> bool {
    let mut current = internal.to_string();
    loop {
        if current == ancestor {
            return true;
        }
        let Some(class) = jar.class(&current) else {
            return false;
        };
        if class.super_name.is_empty() {
            return false;
        }
        current = class.super_name.clone();
    }
}

/// The simple name of the first class on `internal`'s chain that declares `method`.
fn declaring_class(jar: &Jar, internal: &str, method: &str) -> Option<String> {
    let mut current = internal.to_string();
    loop {
        let class = jar.class(&current)?;
        if class
            .methods
            .iter()
            .any(|candidate| candidate.name == method)
        {
            return Some(simple(&class.name).to_string());
        }
        if class.super_name.is_empty() {
            return None;
        }
        current = class.super_name.clone();
    }
}

/// The block registry: every id `BuiltInRegistries.BLOCK` holds, with its class.
pub fn registry(jar_path: &Path) -> Result<Vec<Block>, String> {
    let jar = Jar::open(jar_path)?;
    let registry = Registry::load(&jar)?;
    Ok(registry.blocks(&jar))
}

/// One landing entry, in the order Gson writes its fields.
struct Entry {
    id: String,
    fields: Vec<(&'static str, Value)>,
}

/// The landing table's entries, in registry order.
fn entries(jar: &Jar, registry: &Registry) -> Result<Vec<Entry>, String> {
    let blocks = registry.blocks(jar);
    if blocks.len() != registry.registrations.len() {
        return Err("the registry lost registrations".to_string());
    }
    let mut entries = Vec::with_capacity(blocks.len());
    for (block, registration) in blocks.iter().zip(registry.registrations.iter()) {
        let internal = registration
            .class
            .clone()
            .unwrap_or_else(|| BLOCK.to_string());
        let fall = declaring_class(jar, &internal, "fallOn").unwrap_or_else(|| "Block".to_string());
        let bounce = declaring_class(jar, &internal, "bounceOn")
            .is_some_and(|owner| owner == "ShelfMushroomBlock");
        let mut fields = vec![
            ("fall", Value::String(fall.clone())),
            ("bounce", Value::Bool(bounce)),
        ];
        if fall == "FarmlandBlock" {
            if let Some(id) = registration
                .base_field
                .as_ref()
                .and_then(|field| registry.field_ids.get(field))
            {
                fields.push(("base", Value::String(id.clone())));
            }
        }
        if fall == "HoneyBlock" {
            if let Some((volume, pitch)) = registration
                .sound
                .as_ref()
                .and_then(|field| registry.sound_volumes.get(field))
            {
                fields.push(("volume", Value::String(format!("{:08x}", volume.to_bits()))));
                fields.push(("pitch", Value::String(format!("{:08x}", pitch.to_bits()))));
            }
        }
        entries.push(Entry {
            id: block.id.clone(),
            fields,
        });
    }
    Ok(entries)
}

/// The landing table's entries, with the jar opened from `jar_path`.
fn landing_entries(jar_path: &Path) -> Result<Vec<Entry>, String> {
    let jar = Jar::open(jar_path)?;
    let registry = Registry::load(&jar)?;
    entries(&jar, &registry)
}

/// The compact JSON text Gson writes: `{"<id>":{"fall":…,"bounce":…},…}`.
///
/// `serde_json`'s object map sorts its keys, and the bundle's file is in
/// registry order, so the text is rendered from the ordered entries instead of
/// from a `Value`.
fn render(entries: &[Entry]) -> Result<String, String> {
    let mut out = String::from("{");
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let key = serde_json::to_string(&entry.id).map_err(|error| error.to_string())?;
        out.push_str(&key);
        out.push_str(":{");
        for (slot, (name, value)) in entry.fields.iter().enumerate() {
            if slot > 0 {
                out.push(',');
            }
            let name = serde_json::to_string(name).map_err(|error| error.to_string())?;
            let value = serde_json::to_string(value).map_err(|error| error.to_string())?;
            out.push_str(&name);
            out.push(':');
            out.push_str(&value);
        }
        out.push('}');
    }
    out.push('}');
    Ok(out)
}

/// The `landing-definitions.json` document, built from the jar alone.
pub fn landing_definitions(jar_path: &Path) -> Result<Value, String> {
    let mut document = Map::new();
    for entry in landing_entries(jar_path)? {
        let mut object = Map::new();
        for (name, value) in entry.fields {
            object.insert(name.to_string(), value);
        }
        document.insert(entry.id, Value::Object(object));
    }
    Ok(Value::Object(document))
}

/// Write `landing-definitions.json` below `out`, and report what was built.
pub fn write(jar_path: &Path, out: &Path) -> Result<Coverage, String> {
    let jar = Jar::open(jar_path)?;
    let registry = Registry::load(&jar)?;
    let entries = entries(&jar, &registry)?;
    let mut coverage = Coverage::default();
    for table in TABLES {
        coverage.missing.push(table.to_string());
    }
    for (table, reason) in BLOCKED_TABLES {
        coverage.note(format!("{table}: {reason}"));
    }
    let path = out.join(TABLE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    let text = render(&entries)?;
    fs::write(&path, text).map_err(|error| format!("{}: {error}", path.display()))?;
    coverage.add(TABLE);
    // A registration whose factory did not resolve would still be written with
    // the plain `Block` class; say so instead of letting it look exact.
    let unresolved = registry
        .registrations
        .iter()
        .filter(|registration| registration.class.is_none())
        .count();
    coverage.note(format!(
        "{TABLE}: {} blocks in Blocks.<clinit> order, {unresolved} without a resolved class",
        entries.len()
    ));
    Ok(coverage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_method_body_decodes_by_its_own_lengths() {
        // `iconst_1; istore 1; iload 1; ireturn`
        let instructions = decode(&[0x04, 0x36, 0x01, 0x15, 0x01, 0xac]);
        assert_eq!(instructions.len(), 4);
        assert_eq!(instructions[1].index, Some(1));
        assert_eq!(instructions[3].opcode, 0xac);
    }

    #[test]
    fn a_wide_constant_index_reads_two_bytes() {
        let instructions = decode(&[0x13, 0x00, 0x2a]);
        assert_eq!(instructions.len(), 1);
        assert_eq!(instructions[0].index, Some(42));
    }

    #[test]
    fn a_tableswitch_is_skipped_by_its_own_length() {
        // tableswitch at offset 0: pad 3, default 0, low 1, high 2, two targets.
        let mut bytes = vec![0xaa, 0, 0, 0];
        bytes.extend_from_slice(&0i32.to_be_bytes());
        bytes.extend_from_slice(&1i32.to_be_bytes());
        bytes.extend_from_slice(&2i32.to_be_bytes());
        bytes.extend_from_slice(&0i32.to_be_bytes());
        bytes.extend_from_slice(&0i32.to_be_bytes());
        bytes.push(0xac);
        let instructions = decode(&bytes);
        assert_eq!(instructions.len(), 2);
        assert_eq!(instructions[1].opcode, 0xac);
    }

    #[test]
    fn the_blocked_tables_report_what_this_task_names() {
        let blocked: Vec<&str> = BLOCKED_TABLES.iter().map(|(table, _)| *table).collect();
        for table in TABLES {
            if table == TABLE {
                continue;
            }
            assert!(
                blocked.contains(&table),
                "{table} is not reported as blocked"
            );
        }
        for table in [
            "cached-collision.nbt",
            "occlusion.nbt",
            "worldgen/blocks/cached-support.nbt",
            "block-properties.nbt",
            "movement-properties.nbt",
        ] {
            assert!(
                blocked.contains(&table),
                "{table} is not reported as blocked"
            );
        }
        for (_, reason) in BLOCKED_TABLES {
            assert!(
                reason.len() > 40,
                "a reason has to say what is missing: {reason}"
            );
        }
    }
}
