//! The world generator's block placement and survival tables, rebuilt from the player's jar.
//!
//! Three files of `ship.hzr3`, all under `worldgen/blocks/`:
//!
//! * `placement-definitions.json` — the `DoublePlantBlock` and `MossyCarpetBlock` instances:
//!   the blocks the generator places in two halves / carpets that cover a corner.
//! * `survival-rules.json` — per block, the rule feature placement checks before it may put
//!   the block down (`Always`, `SolidBelow`, `TagBelow:minecraft:supports_vegetation`, ...).
//! * `survival-extra-definitions.json` — the fence gate blocks and `FireBlock`'s ignition
//!   odds per block.
//!
//! The Java side is two exporters and a binder:
//!
//! * `VanillaSimpleBlockOracle`'s `export` mode writes `placement-definitions.json`
//!   (`block instanceof DoublePlantBlock` / `MossyCarpetBlock`, in registry order).
//! * `VanillaBlockSurvivalOracle`'s `export` mode writes `survival-extra-definitions.json`
//!   (`block instanceof FenceGateBlock`, plus `FireBlock.igniteOdds` read off the fire
//!   block) and three intermediates the binder reads: for every block the class that
//!   *declares* `canSurvive`, the class that declares `mayPlaceOn`, and the block's
//!   `TagKey` fields.
//! * `horizon-prepare`'s `survival::bind_definitions` (Rust) turns those intermediates into
//!   `survival-rules.json` with its two class tables.
//!
//! This module does the same work directly from the jar. The declaring class comes from the
//! block's class chain (the walk `Class.getDeclaredMethod` does); the tag ids come from
//! `BlockTags`/`BlockItemTags`; the ignition odds from `FireBlock.bootStrap()`'s table (the
//! long run of `setFlammable` calls plus the four `ColorCollection` loops, where the coloured
//! wool, wool stairs, wool slabs and carpets are filled in); the two class → rule tables are
//! the binder's own ([`CLASS_RULES`], [`VEGETATION_RULES`]).
//!
//! Nothing here reads `ship.hzr3`.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

#[path = "../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;

#[path = "block.rs"]
#[allow(dead_code)]
mod block;

use classfile::ClassFile;

/// The three files, in the order the bundle holds them.
pub const TABLES: [&str; 3] = [
    "worldgen/blocks/placement-definitions.json",
    "worldgen/blocks/survival-rules.json",
    "worldgen/blocks/survival-extra-definitions.json",
];

/// What was produced, and what is still out of reach.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub produced: Vec<String>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
}

/// The three tables' contents.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tables {
    /// `DoublePlantBlock` instances, in registry order.
    pub double_plants: Vec<String>,
    /// `MossyCarpetBlock` instances, in registry order.
    pub mossy_carpets: Vec<String>,
    /// block id -> survival rule, in the order the bundle inserts them.
    pub survival: Vec<(String, String)>,
    /// `FenceGateBlock` instances, in registry order.
    pub fence_gates: Vec<String>,
    /// Every block's ignition odds, in registry order.
    pub ignition_odds: Vec<(String, i32)>,
}

// ---------------------------------------------------------------------------
// The binder's tables, from horizon-prepare's `survival.rs`
// ---------------------------------------------------------------------------

/// The rule of the class that declares `canSurvive`, in the binder's lookup order.
const CLASS_RULES: [(&str, &str); 61] = [
    ("FireBlock", "Fire"),
    ("CactusBlock", "Cactus"),
    ("PathBlock", "Path"),
    ("MushroomBlock", "Mushroom"),
    ("MultifaceBlock", "Multiface"),
    ("VineBlock", "Vine"),
    ("HangingMossBlock", "HangingMoss"),
    ("SeaPickleBlock", "SeaPickle"),
    ("SnowLayerBlock", "SnowLayer"),
    ("BellBlock", "Bell"),
    ("ShelfMushroomBlock", "FullBehind"),
    ("GrowingPlantBlock", "GrowingPlant"),
    ("MangrovePropaguleBlock", "MangrovePropagule"),
    ("SpeleothemBlock", "Speleothem"),
    ("ScaffoldingBlock", "Scaffolding"),
    ("PistonHeadBlock", "PistonHead"),
    ("ChorusPlantBlock", "ChorusPlant"),
    ("ChorusFlowerBlock", "ChorusFlower"),
    ("PitcherCropBlock", "PitcherCrop"),
    ("BigDripleafBlock", "BigDripleaf"),
    ("BigDripleafStemBlock", "BigDripleafStem"),
    ("FarmlandBlock", "Farmland"),
    ("CocoaBlock", "Cocoa"),
    ("SugarCaneBlock", "SugarCane"),
    ("SporeBlossomBlock", "SporeBlossom"),
    ("FrogspawnBlock", "Frogspawn"),
    ("SmallDripleafBlock", "SmallDripleaf"),
    ("TallSeagrassBlock", "TallSeagrass"),
    ("DoorBlock", "Door"),
    ("BaseRailBlock", "RigidBelow"),
    ("LeafLitterBlock", "FullBelow"),
    ("WallSignBlock", "SolidBehind"),
    ("GrindstoneBlock", "Always"),
    ("RedstoneWallTorchBlock", "FullBehind"),
    ("BubbleColumnBlock", "BubbleColumn"),
    ("RedstoneWireBlock", "RedstoneWire"),
    ("SoulFireBlock", "TagBelow:minecraft:soul_fire_base_blocks"),
    ("BlockBehaviour", "Always"),
    ("CakeBlock", "SolidBelow"),
    ("CandleCakeBlock", "SolidBelow"),
    ("BannerBlock", "SolidBelow"),
    ("StandingSignBlock", "SolidBelow"),
    ("WallBannerBlock", "SolidBehind"),
    ("CarpetBlock", "NonAirBelow"),
    ("MossyCarpetBlock", "MossCarpet"),
    ("BaseTorchBlock", "CenterBelow"),
    ("CandleBlock", "CenterBelow"),
    ("LanternBlock", "Lantern"),
    ("HangingRootsBlock", "FullAbove"),
    ("LadderBlock", "FullBehind"),
    ("AmethystClusterBlock", "FullBehind"),
    ("TripWireHookBlock", "FullBehind"),
    ("BaseCoralWallFanBlock", "FullBehind"),
    ("WallTorchBlock", "FullBehind"),
    ("BasePressurePlateBlock", "RigidOrCenterBelow"),
    ("DiodeBlock", "RigidBelow"),
    ("BaseCoralPlantTypeBlock", "FullBelow"),
    ("CeilingHangingSignBlock", "CenterAbove"),
    ("FaceAttachedHorizontalDirectionalBlock", "FaceAttached"),
    ("BambooSaplingBlock", "SupportsBamboo"),
    ("BambooStalkBlock", "SupportsBamboo"),
];

/// The rule of a vegetation block, by the class that declares its `mayPlaceOn`.
const VEGETATION_RULES: [(&str, &str); 9] = [
    ("LilyPadBlock", "LilyPad"),
    ("VegetationBlock", "TagBelow:minecraft:supports_vegetation"),
    ("AzaleaBlock", "TagBelow:minecraft:supports_azalea"),
    (
        "DryVegetationBlock",
        "TagBelow:minecraft:supports_dry_vegetation",
    ),
    (
        "NetherSproutsBlock",
        "TagBelow:minecraft:supports_nether_sprouts",
    ),
    ("NetherWartBlock", "TagBelow:minecraft:supports_nether_wart"),
    ("WitherRoseBlock", "TagBelow:minecraft:supports_wither_rose"),
    ("CactusFlowerBlock", "CactusFlower"),
    ("SeagrassBlock", "Seagrass"),
];

/// The classes whose support tag is a constructor argument rather than the class itself.
const CONSTRUCTOR_TAG_CLASSES: [&str; 4] = [
    "AttachedStemBlock",
    "StemBlock",
    "NetherFungusBlock",
    "NetherRootsBlock",
];

fn lookup(table: &[(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    table
        .iter()
        .find(|(class, _)| *class == key)
        .map(|(_, rule)| *rule)
}

/// The two descriptors the binder's `getDeclaredMethod` calls use.
const CAN_SURVIVE: &str = "(Lnet/minecraft/world/level/block/state/BlockState;Lnet/minecraft/world/level/LevelReader;Lnet/minecraft/core/BlockPos;)Z";
const MAY_PLACE_ON: &str = "(Lnet/minecraft/world/level/block/state/BlockState;Lnet/minecraft/world/level/BlockGetter;Lnet/minecraft/core/BlockPos;)Z";

// ---------------------------------------------------------------------------
// The jar
// ---------------------------------------------------------------------------

/// The jar's class files, parsed on demand.
struct Classes {
    files: BTreeMap<String, Vec<u8>>,
    by_short: BTreeMap<String, String>,
}

impl Classes {
    fn open(path: &Path) -> Result<Classes, String> {
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
        Ok(Classes { files, by_short })
    }

    fn class(&self, name: &str) -> Option<ClassFile> {
        let internal = if name.contains('/') {
            self.files.get_key_value(name)?.0.clone()
        } else {
            self.by_short.get(name)?.clone()
        };
        classfile::parse(self.files.get(&internal)?).ok()
    }

    /// The chain from a block's own class up to `Block`, plus `BlockBehaviour`, which is
    /// where the walk `Class.getDeclaredMethod` ends for a block with no override.
    fn chain(&self, row: &block::Block) -> Vec<String> {
        let mut chain: Vec<String> = Vec::with_capacity(row.chain.len() + 2);
        if row.chain.first() != Some(&row.class) {
            chain.push(row.class.clone());
        }
        chain.extend(row.chain.iter().cloned());
        if chain.iter().all(|class| class != "BlockBehaviour") {
            chain.push("BlockBehaviour".to_string());
        }
        chain
    }

    /// The simple name of the first class on the chain that declares `name` as `descriptor`.
    fn declaring(&self, chain: &[String], name: &str, descriptor: &str) -> Option<String> {
        for class in chain {
            let Some(file) = self.class(class) else {
                continue;
            };
            if file
                .methods
                .iter()
                .any(|method| method.name == name && method.descriptor == descriptor)
            {
                return Some(short(&file.name).to_string());
            }
        }
        None
    }
}

fn short(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

// ---------------------------------------------------------------------------
// A minimal instruction decoder
// ---------------------------------------------------------------------------

/// One decoded instruction: its opcode, the constant index it reads, and a small immediate.
struct Instruction {
    opcode: u8,
    index: Option<usize>,
    immediate: Option<i32>,
}

/// Decode a method body. `tableswitch`/`lookupswitch` are skipped by their own length so the
/// scan cannot drift (the same shape `block.rs` uses).
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

/// A `Constant::Class`/`Constant::String` holds `#index`; resolve it to the text.
fn resolve(constants: &[classfile::Constant], reference: &str) -> Option<String> {
    let index: usize = reference.trim_start_matches('#').parse().ok()?;
    match constants.get(index)? {
        classfile::Constant::Utf8(text) => Some(text.clone()),
        _ => None,
    }
}

/// `(owner, member name, descriptor)` behind a field or method reference.
fn member_at(constants: &[classfile::Constant], index: usize) -> Option<(String, String, String)> {
    let (class, name_and_type) = match constants.get(index)? {
        classfile::Constant::Fieldref { class, name, .. } => (class, name),
        classfile::Constant::Methodref { class, name, .. } => (class, name),
        classfile::Constant::InterfaceMethodref { class, name, .. } => (class, name),
        _ => return None,
    };
    let class_index: usize = class.trim_start_matches('#').parse().ok()?;
    let owner = resolve(
        constants,
        match constants.get(class_index)? {
            classfile::Constant::Class(reference) => reference,
            _ => return None,
        },
    )?;
    let name_index: usize = name_and_type.trim_start_matches('#').parse().ok()?;
    let (member, descriptor) = match constants.get(name_index)? {
        classfile::Constant::NameAndType { name, kind } => (name, kind),
        _ => return None,
    };
    Some((
        owner,
        resolve(constants, member)?,
        resolve(constants, descriptor)?,
    ))
}

/// The implementation method an `invokedynamic` names, with the class that holds it.
fn lambda_at(class: &ClassFile, index: usize) -> Option<(String, String)> {
    let bootstrap = match class.constants.get(index)? {
        classfile::Constant::InvokeDynamic { bootstrap, .. } => *bootstrap,
        _ => return None,
    };
    let row = class.bootstraps.get(bootstrap)?;
    for argument in &row.arguments {
        let Some(classfile::Constant::MethodHandle { reference }) = class.constants.get(*argument)
        else {
            continue;
        };
        let Some(classfile::Constant::Methodref {
            class: owner, name, ..
        }) = class.constants.get(*reference)
        else {
            continue;
        };
        let owner_index: usize = owner.trim_start_matches('#').parse().ok()?;
        let Some(owner) = class_at(&class.constants, owner_index) else {
            continue;
        };
        let name_index: usize = name.trim_start_matches('#').parse().ok()?;
        let Some(classfile::Constant::NameAndType { name, .. }) = class.constants.get(name_index)
        else {
            continue;
        };
        return Some((owner, resolve(&class.constants, name)?));
    }
    None
}

/// The class a `Constant::Class` pool index names.
fn class_at(constants: &[classfile::Constant], index: usize) -> Option<String> {
    match constants.get(index)? {
        classfile::Constant::Class(reference) => resolve(constants, reference),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Building the tables
// ---------------------------------------------------------------------------

/// Builds all three tables from the player's jar.
pub fn tables(jar_path: &Path) -> Result<Tables, String> {
    let registry = block::registry(jar_path)?;
    let classes = Classes::open(jar_path)?;
    let tags = support_tags(&classes, &registry)?;
    let odds = ignition_odds(&classes, &registry)?;
    let mut out = Tables::default();
    let mut classified = Vec::with_capacity(registry.len());
    for row in &registry {
        let chain = classes.chain(row);
        let is = |class: &str| chain.iter().any(|candidate| candidate == class);
        if is("DoublePlantBlock") {
            out.double_plants.push(row.id.clone());
        }
        if is("MossyCarpetBlock") {
            out.mossy_carpets.push(row.id.clone());
        }
        if is("FenceGateBlock") {
            out.fence_gates.push(row.id.clone());
        }
        out.ignition_odds
            .push((row.id.clone(), odds.get(&row.id).copied().unwrap_or(0)));
        let owner = classes
            .declaring(&chain, "canSurvive", CAN_SURVIVE)
            .ok_or_else(|| format!("{}: no class declares canSurvive", row.id))?;
        let placed = classes.declaring(&chain, "mayPlaceOn", MAY_PLACE_ON);
        classified.push((row.id.clone(), owner, placed));
    }
    // The binder's two passes: every block the class table names, in registry order, then
    // the crops and vegetation it classifies from the placement class. A `put` on a key
    // that is already there keeps its position, as Gson's object does.
    let mut survival: Vec<(String, String)> = Vec::new();
    let mut positions: BTreeMap<String, usize> = BTreeMap::new();
    for (id, owner, _) in &classified {
        if let Some(rule) = lookup(&CLASS_RULES, owner) {
            positions.insert(id.clone(), survival.len());
            survival.push((id.clone(), rule.to_string()));
        }
    }
    for (id, owner, placed) in &classified {
        let rule = match (owner.as_str(), placed.as_deref()) {
            ("CropBlock", Some("CropBlock")) => Some("Crop".to_string()),
            ("DoublePlantBlock", Some("VegetationBlock")) => Some("DoublePlant".to_string()),
            ("VegetationBlock", placed) => {
                let placed = placed
                    .ok_or_else(|| format!("{id}: no placement class for a vegetation block"))?;
                if let Some(rule) = lookup(&VEGETATION_RULES, placed) {
                    Some(rule.to_string())
                } else if CONSTRUCTOR_TAG_CLASSES.contains(&placed) {
                    let tag = tags
                        .get(id)
                        .ok_or_else(|| format!("{id}: no support tag for {placed}"))?;
                    Some(format!("TagBelow:{tag}"))
                } else {
                    None
                }
            }
            _ => None,
        };
        let Some(rule) = rule else { continue };
        match positions.get(id) {
            Some(slot) => survival[*slot].1 = rule,
            None => {
                positions.insert(id.clone(), survival.len());
                survival.push((id.clone(), rule));
            }
        }
    }
    out.survival = survival;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Support tags
// ---------------------------------------------------------------------------

/// `BlockItemTags`: field name -> `minecraft:<tag>`.
fn block_item_tags(classes: &Classes) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(file) = classes.class("net/minecraft/tags/BlockItemTags") else {
        return out;
    };
    let Some(code) = file
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
                    .and_then(|index| file.constants.get(index))
                {
                    id = resolve(&file.constants, reference);
                }
            }
            0xb3 => {
                if let Some((owner, field, _)) = instruction
                    .index
                    .and_then(|index| member_at(&file.constants, index))
                {
                    if owner == "net/minecraft/tags/BlockItemTags" {
                        if let Some(id) = id.take() {
                            out.insert(field, tag_id(&id));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// `BlockTags`: field name -> `minecraft:<tag>`.
fn block_tags(class: &ClassFile, items: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
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
                    id = resolve(&class.constants, reference);
                }
            }
            0xb2 => {
                if let Some((owner, field, _)) = instruction
                    .index
                    .and_then(|index| member_at(&class.constants, index))
                {
                    if owner == "net/minecraft/tags/BlockItemTags" {
                        id = items.get(&field).cloned();
                    }
                }
            }
            0xb3 => {
                if let Some((owner, field, _)) = instruction
                    .index
                    .and_then(|index| member_at(&class.constants, index))
                {
                    if owner == "net/minecraft/tags/BlockTags" {
                        if let Some(id) = id.take() {
                            out.insert(field, tag_id(&id));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// A tag id with the default namespace filled in, as TagKey.create does.
fn tag_id(name: &str) -> String {
    if name.contains(':') {
        name.to_string()
    } else {
        format!("minecraft:{name}")
    }
}

/// Block id -> the support tag its constructor is given, for the four classes whose support
/// tag is a constructor argument.
fn support_tags(
    classes: &Classes,
    registry: &[block::Block],
) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    let items = block_item_tags(classes);
    let Some(tags_class) = classes.class("net/minecraft/tags/BlockTags") else {
        return Ok(out);
    };
    let tags = block_tags(&tags_class, &items);
    let Some(blocks) = classes.class("net/minecraft/world/level/block/Blocks") else {
        return Ok(out);
    };
    let Some(code) = blocks
        .methods
        .iter()
        .find(|method| method.name == "<clinit>")
        .and_then(|method| method.code.as_ref())
    else {
        return Err("Blocks.<clinit> has no code".into());
    };
    // A registration is getstatic BlockItemIds.X, factory, properties, register,
    // putstatic FIELD; a block's field name is its id in upper case, and the factory is the
    // last lambda the registration made.
    let known: BTreeMap<String, ()> = registry.iter().map(|row| (row.id.clone(), ())).collect();
    let mut lambda: Option<String> = None;
    let mut by_id: BTreeMap<String, String> = BTreeMap::new();
    for instruction in decode(&code.bytes) {
        match instruction.opcode {
            0xba => {
                if let Some((_, target)) = instruction
                    .index
                    .and_then(|index| lambda_at(&blocks, index))
                {
                    if blocks
                        .methods
                        .iter()
                        .find(|method| method.name == target)
                        .map(|method| {
                            method
                                .descriptor
                                .ends_with(")Lnet/minecraft/world/level/block/Block;")
                        })
                        .unwrap_or(false)
                    {
                        lambda = Some(target);
                    }
                }
            }
            0xb3 => {
                if let Some((owner, field, descriptor)) = instruction
                    .index
                    .and_then(|index| member_at(&blocks.constants, index))
                {
                    if owner == "net/minecraft/world/level/block/Blocks"
                        && descriptor == "Lnet/minecraft/world/level/block/Block;"
                    {
                        if let Some(target) = lambda.take() {
                            let id = format!("minecraft:{}", field.to_lowercase());
                            if known.contains_key(&id) {
                                by_id.insert(id, target);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    for (id, method) in by_id {
        let Some(method) = blocks
            .methods
            .iter()
            .find(|candidate| candidate.name == method)
        else {
            continue;
        };
        let Some(code) = method.code.as_ref() else {
            continue;
        };
        for instruction in decode(&code.bytes) {
            if instruction.opcode != 0xb2 {
                continue;
            }
            if let Some((owner, field, descriptor)) = instruction
                .index
                .and_then(|index| member_at(&blocks.constants, index))
            {
                if owner == "net/minecraft/tags/BlockTags"
                    && descriptor == "Lnet/minecraft/tags/TagKey;"
                {
                    if let Some(tag) = tags.get(&field) {
                        out.insert(id.clone(), tag.clone());
                        break;
                    }
                }
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Ignition odds
// ---------------------------------------------------------------------------

/// The sixteen dye colour words, from `DyeColor`'s own constants.
fn dye_colors(classes: &Classes) -> Vec<String> {
    let Some(class) = classes.class("net/minecraft/world/item/DyeColor") else {
        return Vec::new();
    };
    class
        .fields
        .iter()
        .filter(|field| field.descriptor == "Lnet/minecraft/world/item/DyeColor;")
        .map(|field| field.name.to_lowercase())
        .collect()
}

/// The ignition odds a `ColorCollection.forEach` lambda passes to `setFlammable`.
fn lambda_ignition(class: &ClassFile, method: &str) -> Option<i32> {
    let method = class
        .methods
        .iter()
        .find(|candidate| candidate.name == method)?;
    let code = method.code.as_ref()?;
    // `setFlammable(fire, block, ignite, burn)`: the two odds are the last two constants
    // pushed before the call.
    let mut numbers = Vec::new();
    for instruction in decode(&code.bytes) {
        if instruction.opcode == 0xb6 {
            if let Some((_, name, _)) = instruction
                .index
                .and_then(|index| member_at(&class.constants, index))
            {
                if name == "setFlammable" {
                    return numbers.iter().rev().nth(1).copied();
                }
            }
        }
        if let Some(value) = instruction.immediate {
            numbers.push(value);
        }
    }
    None
}

/// `FireBlock`'s `igniteOdds`: the per-block table the fire block is built with.
///
/// It is filled two ways: a long run of `setFlammable(Blocks.X, ignite, burn)` calls, and
/// four `Blocks.<colour collection>.forEach(block -> setFlammable(...))` loops (wool, wool
/// stairs, wool slabs, carpets), which is where the coloured blocks get their odds.
fn ignition_odds(
    classes: &Classes,
    registry: &[block::Block],
) -> Result<BTreeMap<String, i32>, String> {
    let mut out = BTreeMap::new();
    let Some(fire) = classes.class("net/minecraft/world/level/block/FireBlock") else {
        return Ok(out);
    };
    // The table is filled from `bootStrap()` (called by `Bootstrap`) as well as from the
    // constructor, so every method body is read.
    let bodies: Vec<&classfile::Code> = fire
        .methods
        .iter()
        .filter_map(|method| method.code.as_ref())
        .collect();
    if bodies.is_empty() {
        return Err("FireBlock has no code".into());
    }
    let known: BTreeMap<String, ()> = registry.iter().map(|row| (row.id.clone(), ())).collect();
    let colors = dye_colors(classes);
    for code in bodies {
        let mut block: Option<String> = None;
        let mut numbers: Vec<i32> = Vec::new();
        let mut collection: Option<String> = None;
        let mut consumer: Option<String> = None;
        for instruction in decode(&code.bytes) {
            match instruction.opcode {
                0xb2 => {
                    if let Some((owner, field, descriptor)) = instruction
                        .index
                        .and_then(|index| member_at(&fire.constants, index))
                    {
                        if owner == "net/minecraft/world/level/block/Blocks" {
                            if descriptor == "Lnet/minecraft/world/level/block/Block;" {
                                block = Some(format!("minecraft:{}", field.to_lowercase()));
                                numbers.clear();
                            } else if descriptor
                                == "Lnet/minecraft/world/level/block/ColorCollection;"
                            {
                                collection = Some(field.to_lowercase());
                            }
                        }
                    }
                }
                0xba => {
                    if let Some((_, target)) =
                        instruction.index.and_then(|index| lambda_at(&fire, index))
                    {
                        consumer = Some(target);
                    }
                }
                0xb6 => {
                    if let Some((_, name, _)) = instruction
                        .index
                        .and_then(|index| member_at(&fire.constants, index))
                    {
                        if name == "setFlammable" {
                            if let (Some(block), Some(ignite)) = (block.take(), numbers.first()) {
                                out.insert(block, *ignite);
                            }
                            numbers.clear();
                        }
                        if name == "forEach" {
                            if let (Some(collection), Some(consumer)) =
                                (collection.take(), consumer.take())
                            {
                                if let Some(ignite) = lambda_ignition(&fire, &consumer) {
                                    for color in &colors {
                                        let id = format!("minecraft:{color}_{collection}");
                                        if known.contains_key(&id) {
                                            out.insert(id, ignite);
                                        }
                                    }
                                }
                            }
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
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The reports
// ---------------------------------------------------------------------------

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_array(values: &[String]) -> String {
    let mut out = String::from("[");
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&json_string(value));
    }
    out.push(']');
    out
}

/// `placement-definitions.json`, the way Gson writes it.
pub fn render_placement(tables: &Tables) -> String {
    format!(
        "{{\"double_plants\":{},\"mossy_carpets\":{}}}",
        json_array(&tables.double_plants),
        json_array(&tables.mossy_carpets)
    )
}

/// `survival-rules.json`, the way Gson writes it.
pub fn render_survival(tables: &Tables) -> String {
    let mut out = String::from("{");
    for (index, (block, rule)) in tables.survival.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&json_string(block));
        out.push(':');
        out.push_str(&json_string(rule));
    }
    out.push('}');
    out
}

/// `survival-extra-definitions.json`, the way Gson writes it.
pub fn render_extra(tables: &Tables) -> String {
    let mut out = format!(
        "{{\"fence_gates\":{},\"ignition_odds\":{{",
        json_array(&tables.fence_gates)
    );
    for (index, (block, odds)) in tables.ignition_odds.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&json_string(block));
        out.push(':');
        out.push_str(&odds.to_string());
    }
    out.push_str("}}");
    out
}

/// Builds the three tables and writes them below `root`.
pub fn write(jar_path: &Path, root: &Path) -> Result<Coverage, String> {
    let tables = tables(jar_path)?;
    let rendered = [
        render_placement(&tables),
        render_survival(&tables),
        render_extra(&tables),
    ];
    for (name, text) in TABLES.iter().zip(rendered) {
        let path = root.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let mut coverage = Coverage::default();
    coverage
        .produced
        .extend(TABLES.iter().map(|name| name.to_string()));
    Ok(coverage)
}
