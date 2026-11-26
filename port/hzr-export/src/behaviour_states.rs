//! Rebuild `mob-data.json#states` from the player's own `client.jar`.
//!
//! `VanillaMobDataExporter.states()` writes one row per registry state:
//!
//! ```text
//! [WalkNodeEvaluator.getPathTypeFromState(level, ZERO), flags, collision height]
//! ```
//!
//! with `level` = a getter holding the state at the origin, and the flag mask
//! `1 LAND|2 WATER|4 AIR|8 zombie|16 ocelot|32 polar bear|64 redstone|
//! 128 empty-spawn|256 face-sturdy-up|512 solid|1024 solid-render`.
//!
//! Every fact is derived from the jar:
//!
//! * collision / occlusion / support shapes: `shape_final::evaluate` (the shape
//!   engines' final integration), plus `geometry::Shape` predicates;
//! * `hasCollision`, `forceSolidOn/Off`: `shape_flags` / `shape` (the block
//!   property tables);
//! * path type: `WalkNodeEvaluator.getPathTypeFromState` ported branch for
//!   branch, with the block/fluid tags read from the jar's data pack;
//! * the per-block `Properties` predicates (`isValidSpawn`,
//!   `isRedstoneConductor`, `lightLevel`) and `ofLegacyCopy` inheritance: a
//!   stack machine over `Blocks.<clinit>`;
//! * `isPathfindable` / `isSignalSource` / `getFluidState`: the override table
//!   read from the block classes' `Test`/body shape (one branch per declaring
//!   class), with the per-state property values derived from the state table.
//!
//! The module is included by path from `tests/mob_states.rs`, which supplies
//! `crate::classfile` (the reader shared with `hzr-bytecode`).

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::{Map, Value};

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

use classfile::ClassFile;

#[path = "shape_final.rs"]
mod shape_final;

#[path = "voxelshape.rs"]
mod voxelshape;

#[path = "shape.rs"]
mod shape_props;

#[path = "shape_flags.rs"]
mod shape_flags;

use shape_final::block as blockmod;
use shape_final::geometry;
use shape_final::states as statemod;

// ---------------------------------------------------------------------------
// tags
// ---------------------------------------------------------------------------

struct Tags {
    sets: BTreeMap<String, BTreeSet<String>>,
}

impl Tags {
    fn load(jar_path: &Path) -> Result<Self, String> {
        let file = fs::File::open(jar_path).map_err(|e| format!("{}: {e}", jar_path.display()))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", jar_path.display()))?;
        let mut raw: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().map_err(|e| e.to_string())?.to_string();
            let Some(rest) = name.strip_prefix("data/minecraft/tags/block/") else {
                continue;
            };
            let Some(id) = rest.strip_suffix(".json") else {
                continue;
            };
            let mut data = Vec::new();
            use std::io::Read;
            entry.read_to_end(&mut data).map_err(|e| e.to_string())?;
            let value: Value =
                serde_json::from_slice(&data).map_err(|e| format!("{name}: {e}"))?;
            let mut members = Vec::new();
            if let Some(values) = value.get("values").and_then(Value::as_array) {
                for member in values {
                    if let Some(text) = member.as_str() {
                        members.push(text.to_string());
                    }
                }
            }
            raw.insert(format!("minecraft:{id}"), members);
        }
        let mut sets = BTreeMap::new();
        for (id, _) in &raw {
            let mut done = BTreeSet::new();
            let mut seen = BTreeSet::new();
            resolve_tag(id, &raw, &mut done, &mut seen);
            sets.insert(id.clone(), done);
        }
        Ok(Self { sets })
    }

    fn has(&self, tag: &str, block: &str) -> bool {
        self.sets
            .get(&format!("minecraft:{tag}"))
            .is_some_and(|members| members.contains(block))
    }
}

fn resolve_tag(
    id: &str,
    raw: &BTreeMap<String, Vec<String>>,
    done: &mut BTreeSet<String>,
    seen: &mut BTreeSet<String>,
) {
    if !seen.insert(id.to_string()) {
        return;
    }
    let Some(members) = raw.get(id) else { return };
    for member in members {
        if let Some(nested) = member.strip_prefix('#') {
            let nested = if nested.contains(':') {
                nested.to_string()
            } else {
                format!("minecraft:{nested}")
            };
            let mut inner = BTreeSet::new();
            let mut inner_seen = seen.clone();
            resolve_tag(&nested, raw, &mut inner, &mut inner_seen);
            done.extend(inner);
        } else {
            done.insert(member.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// shape predicates
// ---------------------------------------------------------------------------

fn shape_is_empty(shape: &geometry::Shape) -> bool {
    shape.occupied.iter().all(|value| *value == 0)
}

/// Point-set `Block.isShapeFullBlock`: the shape covers the whole unit cube's
/// discretised cells.
fn shape_is_full(shape: &geometry::Shape) -> bool {
    if shape_is_empty(shape) {
        return false;
    }
    let xs = shape.x.len().saturating_sub(1);
    let ys = shape.y.len().saturating_sub(1);
    let zs = shape.z.len().saturating_sub(1);
    if xs == 0 || ys == 0 || zs == 0 {
        return false;
    }
    if shape.x.first() != Some(&0.0)
        || shape.x.last() != Some(&1.0)
        || shape.y.first() != Some(&0.0)
        || shape.y.last() != Some(&1.0)
        || shape.z.first() != Some(&0.0)
        || shape.z.last() != Some(&1.0)
    {
        return false;
    }
    // The prepared tables store `occupied` as `(x * ySize + y) * zSize + z`.
    (0..ys).all(|y| {
        (0..zs).all(|z| {
            (0..xs).all(|x| {
                let index = (x * ys + y) * zs + z;
                shape.occupied.get(index).copied().unwrap_or(0) != 0
            })
        })
    })
}

/// `Block.isFaceFull(shape, UP)`: the top cell layer covers the whole XZ square.
fn shape_face_full_up(shape: &geometry::Shape) -> bool {
    if shape_is_empty(shape) {
        return false;
    }
    if shape_is_full(shape) {
        return true;
    }
    let xs = shape.x.len().saturating_sub(1);
    let ys = shape.y.len().saturating_sub(1);
    let zs = shape.z.len().saturating_sub(1);
    if xs == 0 || ys == 0 || zs == 0 {
        return false;
    }
    // `calculateFace` returns the shape itself when it is cube-like along the
    // axis; a cube-like shape that is not the full block is not a full face.
    if shape.y.len() == 2 && shape.y[0] == 0.0 && shape.y[1] == 1.0 {
        return false;
    }
    // `findIndex(0.9999999)` is the index of the last coordinate below the
    // value: the cell layer the top face is taken from.
    let mut below = 0usize;
    for coordinate in &shape.y {
        if *coordinate <= 0.9999999 {
            below += 1;
        } else {
            break;
        }
    }
    if below == 0 {
        return false;
    }
    let y = below - 1;
    if y >= ys {
        return false;
    }
    // `SliceShape` reports the unit interval on the sliced axis but keeps the
    // delegate's coordinate lists on the other two; the slice is the full block
    // only when they span the unit interval and the layer is fully occupied.
    if shape.x.first() != Some(&0.0) || shape.x.last() != Some(&1.0) {
        return false;
    }
    if shape.z.first() != Some(&0.0) || shape.z.last() != Some(&1.0) {
        return false;
    }
    (0..zs).all(|z| {
        (0..xs).all(|x| {
            let index = (x * ys + y) * zs + z;
            shape.occupied.get(index).copied().unwrap_or(0) != 0
        })
    })
}

fn shape_bounds(shape: &geometry::Shape) -> [f64; 6] {
    [
        shape.x.first().copied().unwrap_or(0.0),
        shape.y.first().copied().unwrap_or(0.0),
        shape.z.first().copied().unwrap_or(0.0),
        shape.x.last().copied().unwrap_or(0.0),
        shape.y.last().copied().unwrap_or(0.0),
        shape.z.last().copied().unwrap_or(0.0),
    ]
}

fn shape_height(shape: &Option<geometry::Shape>) -> f64 {
    match shape {
        None => 0.0,
        Some(shape) => {
            if shape_is_empty(shape) {
                0.0
            } else {
                shape.y.last().copied().unwrap_or(0.0)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// state values
// ---------------------------------------------------------------------------

fn state_values(block: &statemod::BlockStates, offset: usize) -> BTreeMap<String, String> {
    let sorted = block.sorted();
    let counts: Vec<usize> = sorted.iter().map(|property| property.values.len()).collect();
    let mut out = BTreeMap::new();
    for (index, property) in sorted.iter().enumerate() {
        let mut product = 1usize;
        for count in counts.iter().skip(index + 1) {
            product = product.saturating_mul(*count);
        }
        if product == 0 {
            continue;
        }
        let value_index = (offset / product) % counts[index].max(1);
        if let Some(value) = property.values.get(value_index) {
            out.insert(property.name.clone(), value.clone());
        }
    }
    out
}

fn property_true(values: &BTreeMap<String, String>, name: &str) -> bool {
    values.get(name).map(String::as_str) == Some("true")
}

fn property_int(values: &BTreeMap<String, String>, name: &str) -> Option<i32> {
    values.get(name).and_then(|value| value.parse().ok())
}

// ---------------------------------------------------------------------------
// class-derived predicates
// ---------------------------------------------------------------------------

const WATERLOGGED_CLASSES: &[&str] = &[
    "AmethystClusterBlock",
    "BarrierBlock",
    "BaseCoralPlantTypeBlock",
    "BaseRailBlock",
    "BigDripleafBlock",
    "BigDripleafStemBlock",
    "CampfireBlock",
    "CandleBlock",
    "ChainBlock",
    "ChestBlock",
    "ConduitBlock",
    "CopperGolemStatueBlock",
    "CrossCollisionBlock",
    "DecoratedPotBlock",
    "DriedGhastBlock",
    "EnderChestBlock",
    "HangingRootsBlock",
    "HeavyCoreBlock",
    "LadderBlock",
    "LanternBlock",
    "LeavesBlock",
    "LightBlock",
    "LightningRodBlock",
    "MangrovePropaguleBlock",
    "MangroveRootsBlock",
    "MultifaceBlock",
    "ScaffoldingBlock",
    "SculkSensorBlock",
    "SculkShriekerBlock",
    "SeaPickleBlock",
    "ShelfBlock",
    "SignBlock",
    "SlabBlock",
    "SmallDripleafBlock",
    "SpeleothemBlock",
    "StairBlock",
    "TrapDoorBlock",
    "WallBlock",
    "WaterloggedTransparentBlock",
];

const ALWAYS_WATER_CLASSES: &[&str] = &[
    "BubbleColumnBlock",
    "KelpBlock",
    "KelpPlantBlock",
    "SeagrassBlock",
    "TallSeagrassBlock",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fluid {
    None,
    Water,
    Lava,
}

fn fluid_state(chain: &[String], block_id: &str, values: &BTreeMap<String, String>) -> Fluid {
    for class in chain {
        if class == "LiquidBlock" {
            let id = block_id.trim_start_matches("minecraft:");
            return if id == "lava" { Fluid::Lava } else { Fluid::Water };
        }
        if WATERLOGGED_CLASSES.contains(&class.as_str()) {
            return if property_true(values, "waterlogged") {
                Fluid::Water
            } else {
                Fluid::None
            };
        }
        if ALWAYS_WATER_CLASSES.contains(&class.as_str()) {
            return Fluid::Water;
        }
        if class == "BlockBehaviour" || class == "Block" {
            break;
        }
    }
    // The default `getFluidState` returns the empty fluid.
    Fluid::None
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pathfindable {
    Default,
    Never,
    Door,
    TrapDoor,
    FenceGate,
    Slab,
    SnowLayer,
    Liquid,
    WaterOnly,
    Vegetation,
}

fn pathfindable_behavior(chain: &[String]) -> Pathfindable {
    for class in chain {
        let behaviour = match class.as_str() {
            "DoorBlock" => Pathfindable::Door,
            "TrapDoorBlock" => Pathfindable::TrapDoor,
            "FenceGateBlock" => Pathfindable::FenceGate,
            "SlabBlock" => Pathfindable::Slab,
            "SnowLayerBlock" => Pathfindable::SnowLayer,
            "LiquidBlock" => Pathfindable::Liquid,
            "CopperGolemStatueBlock" | "ShelfBlock" => Pathfindable::WaterOnly,
            "VegetationBlock" => Pathfindable::Vegetation,
            "BlockBehaviour" | "Block" => Pathfindable::Default,
            other => {
                if declares_is_pathfindable(other) {
                    Pathfindable::Never
                } else {
                    continue;
                }
            }
        };
        return behaviour;
    }
    Pathfindable::Default
}

/// Classes that override `BlockBehaviour.isPathfindable`; all but the special
/// eight return `false`.
fn declares_is_pathfindable(class: &str) -> bool {
    matches!(
        class,
        "AbstractBedBlock"
            | "AbstractCauldronBlock"
            | "AbstractSkullBlock"
            | "AnvilBlock"
            | "AzaleaBlock"
            | "BambooStalkBlock"
            | "BellBlock"
            | "BrewingStandBlock"
            | "CactusBlock"
            | "CakeBlock"
            | "CampfireBlock"
            | "CandleCakeBlock"
            | "ChainBlock"
            | "ChestBlock"
            | "ChorusPlantBlock"
            | "CocoaBlock"
            | "ComposterBlock"
            | "ConduitBlock"
            | "CopperGolemStatueBlock"
            | "CrossCollisionBlock"
            | "DecoratedPotBlock"
            | "DoorBlock"
            | "DragonEggBlock"
            | "DriedGhastBlock"
            | "EnchantingTableBlock"
            | "EndPortalFrameBlock"
            | "EnderChestBlock"
            | "FarmlandBlock"
            | "FenceBlock"
            | "FenceGateBlock"
            | "FlowerPotBlock"
            | "GrindstoneBlock"
            | "HeavyCoreBlock"
            | "HopperBlock"
            | "LanternBlock"
            | "LecternBlock"
            | "LiquidBlock"
            | "MudBlock"
            | "PathBlock"
            | "PowderSnowBlock"
            | "RespawnAnchorBlock"
            | "RodBlock"
            | "SculkSensorBlock"
            | "SeaPickleBlock"
            | "ShelfBlock"
            | "ShelfMushroomBlock"
            | "SlabBlock"
            | "SnifferEggBlock"
            | "SnowLayerBlock"
            | "SoulSandBlock"
            | "SpeleothemBlock"
            | "StairBlock"
            | "StonecutterBlock"
            | "TrapDoorBlock"
            | "VegetationBlock"
            | "WallBlock"
            | "WallHangingSignBlock"
            | "MovingPistonBlock"
            | "PistonBaseBlock"
            | "PistonHeadBlock"
    )
}

fn declares_signal_source(class: &str) -> bool {
    matches!(
        class,
        "BasePressurePlateBlock"
            | "ButtonBlock"
            | "DaylightDetectorBlock"
            | "DetectorRailBlock"
            | "DiodeBlock"
            | "JukeboxBlock"
            | "LecternBlock"
            | "LeverBlock"
            | "LightningRodBlock"
            | "ObserverBlock"
            | "PoweredBlock"
            | "RedstoneTorchBlock"
            | "RedstoneWireBlock"
            | "SculkSensorBlock"
            | "TargetBlock"
            | "TrappedChestBlock"
            | "TripWireHookBlock"
    )
}

fn is_signal_source(chain: &[String]) -> bool {
    chain.iter().any(|class| declares_signal_source(class))
}

// ---------------------------------------------------------------------------
// Blocks.<clinit> property scan (spawn / redstone / light overrides)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SpawnKind {
    Default,
    Never,
    Always,
    OcelotOrParrot,
    PolarBear,
    FireImmune,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RedstoneKind {
    Default,
    Never,
    Always,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LightKind {
    Zero,
    Constant(i32),
    Lit(i32),
    Max(i32),
    SeaPickle,
    Vault,
    TrialSpawner,
    Unknown,
}

#[derive(Clone, Debug, Default)]
struct Overrides {
    spawn: Option<SpawnKind>,
    redstone: Option<RedstoneKind>,
    light: Option<LightKind>,
    instrument: Option<String>,
    air: Option<bool>,
    suffocating: Option<RedstoneKind>,
    copy: Option<usize>,
}

struct Scan {
    blocks: Vec<Overrides>,
    notes: Vec<String>,
}

fn scan_properties(jar_path: &Path, registry: &[blockmod::Block]) -> Result<Scan, String> {
    let file = fs::File::open(jar_path).map_err(|e| format!("{}: {e}", jar_path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", jar_path.display()))?;
    let mut read = |name: &str| -> Result<Vec<u8>, String> {
        use std::io::Read;
        let mut entry = archive.by_name(name).map_err(|e| format!("{name}: {e}"))?;
        let mut data = Vec::new();
        entry.read_to_end(&mut data).map_err(|e| format!("{name}: {e}"))?;
        Ok(data)
    };
    let blocks_bytes = read("net/minecraft/world/level/block/Blocks.class")?;
    let ids_bytes = read("net/minecraft/references/BlockItemIds.class")?;
    let blocks = classfile::parse(&blocks_bytes).map_err(|e| format!("Blocks: {e}"))?;
    let ids_class = classfile::parse(&ids_bytes).map_err(|e| format!("BlockItemIds: {e}"))?;
    let ids = block_item_ids(&ids_class);
    let index_by_id: BTreeMap<&str, usize> = registry
        .iter()
        .enumerate()
        .map(|(index, block)| (block.id.trim_start_matches("minecraft:"), index))
        .collect();
    let mut values: Vec<Overrides> = vec![Overrides::default(); registry.len()];
    let mut notes = Vec::new();
    let clinit = blocks
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
        .map(|code| code.bytes.clone())
        .ok_or_else(|| "Blocks.<clinit> has no code".to_string())?;
    let ins = decode(&clinit)?;
    // `Properties.instrument(...)` is also set inside `Blocks` helper methods
    // (`woodProperties`, `logProperties`, ...), so map each helper to the
    // instrument constant its body assigns.
    let mut helper_instruments: BTreeMap<String, String> = BTreeMap::new();
    for method in &blocks.methods {
        let Some(code) = method.code.as_ref() else {
            continue;
        };
        let Ok(instructions) = decode(&code.bytes) else {
            continue;
        };
        let mut last: Option<String> = None;
        for instruction in &instructions {
            match instruction.op {
                0xb2 => {
                    if let Some((owner, name, _)) =
                        classfile::field_ref(&blocks.constants, instruction.operand as usize)
                    {
                        if owner.ends_with("NoteBlockInstrument") {
                            last = Some(name);
                        }
                    }
                }
                0xb6 => {
                    if let Some((_, name, _)) =
                        classfile::method_ref(&blocks.constants, instruction.operand as usize)
                    {
                        if name == "instrument" {
                            if let Some(value) = last.take() {
                                helper_instruments.insert(method.name.clone(), value);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let mut stack: Vec<ScanVal> = Vec::new();
    let mut last_id: Option<String> = None;
    let mut last_blocks_field: Option<String> = None;
    let mut last_field_assigned: Option<String> = None;
    let mut field_to_index: BTreeMap<String, usize> = BTreeMap::new();
    let mut pending_copy: Option<String> = None;
    for current in &ins {
        let op = current.op;
        match op {
            0x00 => {}
            0x01 => stack.push(ScanVal::Unknown),
            0x02..=0x08 => stack.push(ScanVal::Int(op as i32 - 3)),
            0x09 | 0x0a | 0x0e | 0x0f => stack.push(ScanVal::Unknown),
            0x0b..=0x0d => stack.push(ScanVal::Int(op as i32 - 3)),
            0x10 | 0x11 => stack.push(ScanVal::Int(current.operand)),
            0x12 | 0x13 | 0x14 => stack.push(ScanVal::Unknown),
            0x15..=0x2d => stack.push(ScanVal::Unknown),
            0x2e..=0x35 => {
                pop_n(&mut stack, 2);
                stack.push(ScanVal::Unknown);
            }
            0x36..=0x4e => {
                pop_n(&mut stack, 1);
            }
            0x4f..=0x56 => {
                pop_n(&mut stack, 3);
            }
            0x57 => {
                pop_n(&mut stack, 1);
            }
            0x58 => {
                pop_n(&mut stack, 1);
            }
            0x59 => {
                let value = stack.last().cloned().unwrap_or(ScanVal::Unknown);
                stack.push(value);
            }
            0x5a..=0x5f => {
                let (pops, pushes) = stack_effect(op);
                let count = stack.len().saturating_sub(pops);
                let top: Vec<ScanVal> = stack[count..].to_vec();
                pop_n(&mut stack, pops);
                for _ in 0..pushes {
                    stack.push(top.get(pushes.saturating_sub(1)).cloned().unwrap_or(ScanVal::Unknown));
                }
            }
            0x60..=0x83 => {
                pop_n(&mut stack, 2);
                stack.push(ScanVal::Unknown);
            }
            0x84 => {}
            0x85..=0x93 => {
                pop_n(&mut stack, 1);
                stack.push(ScanVal::Unknown);
            }
            0x94..=0x98 => {
                pop_n(&mut stack, 2);
                stack.push(ScanVal::Unknown);
            }
            0x99..=0xa6 => {
                pop_n(&mut stack, 1);
            }
            0xa7..=0xa9 => {}
            0xac..=0xb1 => {}
            0xb2 => {
                if let Some((owner, name, descriptor)) =
                    classfile::field_ref(&blocks.constants, current.operand as usize)
                {
                    if owner.ends_with("BlockItemIds") {
                        last_id = ids.get(&name).cloned();
                        stack.push(ScanVal::Unknown);
                    } else if owner.ends_with("Blocks")
                        && descriptor.starts_with("Lnet/minecraft/world/level/block/Block;")
                    {
                        stack.push(ScanVal::BlockRef {
                            id: Some(lower(&name)),
                        });
                    } else if owner.ends_with("NoteBlockInstrument") {
                        stack.push(ScanVal::Text(name));
                    } else {
                        stack.push(ScanVal::Unknown);
                    }
                } else {
                    stack.push(ScanVal::Unknown);
                }
            }
            0xb3 => {
                let value = stack.pop().unwrap_or(ScanVal::Unknown);
                if let Some((owner, name, descriptor)) =
                    classfile::field_ref(&blocks.constants, current.operand as usize)
                {
                    if owner.ends_with("Blocks")
                        && descriptor.starts_with("Lnet/minecraft/world/level/block/Block;")
                    {
                        if let ScanVal::Props(props) = value {
                            let id = lower(&name);
                            if let Some(&index) = index_by_id.get(id.as_str()) {
                                values[index] = props;
                            }
                        }
                    }
                }
            }
            0xb4 => {
                pop_n(&mut stack, 1);
                stack.push(ScanVal::Unknown);
            }
            0xb5 => {
                pop_n(&mut stack, 2);
            }
            0xb6 | 0xb9 => {
                let reference =
                    classfile::method_ref(&blocks.constants, current.operand as usize);
                let Some((_, name, descriptor)) = reference else {
                    continue;
                };
                let arguments = pop_args(&mut stack, &descriptor);
                let receiver = stack.pop().unwrap_or(ScanVal::Unknown);
                if let ScanVal::Props(mut props) = receiver {
                    match name.as_str() {
                        "air" => props.air = Some(true),
                        "instrument" => {
                            if let Some(ScanVal::Text(value)) = arguments.first() {
                                props.instrument = Some(value.clone());
                            }
                        }
                        "isSuffocating" => {
                            if let Some(ScanVal::Func(function)) = arguments.first() {
                                props.suffocating =
                                    Some(classify_predicate(&blocks, function));
                            }
                        }
                        "lightLevel" => {
                            let kind = match arguments.first() {
                                Some(ScanVal::Light(value)) => *value,
                                Some(ScanVal::Func(function)) => {
                                    classify_light(&blocks, function)
                                }
                                _ => LightKind::Unknown,
                            };
                            props.light = Some(kind);
                        }
                        "isValidSpawn" => {
                            if let Some(ScanVal::Func(function)) = arguments.first() {
                                props.spawn = Some(classify_spawn(function));
                            }
                        }
                        "isRedstoneConductor" => {
                            if let Some(ScanVal::Func(function)) = arguments.first() {
                                props.redstone = Some(classify_redstone(function));
                            }
                        }
                        _ => {}
                    }
                    stack.push(ScanVal::Props(props));
                } else {
                    if returns_value(&descriptor) {
                        stack.push(ScanVal::Unknown);
                    }
                }
            }
            0xb7 => {
                let reference =
                    classfile::method_ref(&blocks.constants, current.operand as usize);
                let Some((_, name, descriptor)) = reference else {
                    continue;
                };
                let _ = pop_args(&mut stack, &descriptor);
                stack.pop();
                if name != "<init>" && returns_value(&descriptor) {
                    stack.push(ScanVal::Unknown);
                }
            }
            0xb8 => {
                let reference =
                    classfile::method_ref(&blocks.constants, current.operand as usize);
                let Some((owner, name, descriptor)) = reference else {
                    continue;
                };
                let arguments = pop_args(&mut stack, &descriptor);
                if owner.ends_with("Blocks") {
                    if let Some(instrument) = helper_instruments.get(name.as_str()) {
                        if descriptor.ends_with("Properties;") {
                            let mut props = arguments
                                .iter()
                                .find_map(|value| match value {
                                    ScanVal::Props(props) => Some(props.clone()),
                                    _ => None,
                                })
                                .unwrap_or_default();
                            props.instrument = Some(instrument.clone());
                            stack.push(ScanVal::Props(props));
                            continue;
                        }
                    }
                }
                if owner.ends_with("Properties") && name == "of" {
                    stack.push(ScanVal::Props(Overrides::default()));
                    continue;
                }
                if owner.ends_with("Properties")
                    && (name == "ofLegacyCopy" || name == "ofFullCopy")
                {
                    let mut props = Overrides::default();
                    if let Some(ScanVal::BlockRef { id: Some(source) }) = arguments.first() {
                        if let Some(&index) = index_by_id.get(source.as_str()) {
                            props.copy = Some(index);
                        }
                    }
                    stack.push(ScanVal::Props(props));
                    continue;
                }
                if owner.ends_with("Blocks") && name == "litBlockEmission" {
                    let value = match arguments.first() {
                        Some(ScanVal::Int(value)) => *value,
                        _ => 15,
                    };
                    stack.push(ScanVal::Light(LightKind::Lit(value)));
                    continue;
                }
                if owner.ends_with("Blocks")
                    && (name == "always"
                        || name == "never"
                        || name == "ocelotOrParrot"
                        || name.starts_with("lambda$static$"))
                {
                    stack.push(ScanVal::Func(name.clone()));
                    continue;
                }
                if owner.ends_with("Blocks") && returns_block(&descriptor) {
                    let mut props = arguments
                        .iter()
                        .find_map(|value| match value {
                            ScanVal::Props(props) => Some(props.clone()),
                            _ => None,
                        })
                        .unwrap_or_default();
                    if props.copy.is_none() {
                        if let Some(ScanVal::BlockRef { id: Some(source) }) =
                            arguments.iter().rev().find(|value| {
                                matches!(value, ScanVal::BlockRef { .. })
                            })
                        {
                            if let Some(&index) = index_by_id.get(source.as_str()) {
                                props.copy = Some(index);
                            }
                        }
                    }
                    if let Some(instrument) = helper_instruments.get(name.as_str()) {
                        props.instrument = Some(instrument.clone());
                    }
                    stack.push(ScanVal::Props(props));
                    continue;
                }
                if returns_value(&descriptor) {
                    stack.push(ScanVal::Unknown);
                }
            }
            0xba => {
                let function = indy_target(&blocks, current.operand as usize)
                    .unwrap_or_else(|| "?".to_string());
                let descriptor = indy_descriptor(&blocks, current.operand as usize);
                pop_n(&mut stack, classfile::descriptor_types(&descriptor).len().saturating_sub(1));
                stack.push(ScanVal::Func(function));
            }
            0xbb => stack.push(ScanVal::Unknown),
            0xbc | 0xbd => {
                pop_n(&mut stack, 1);
                stack.push(ScanVal::Unknown);
            }
            0xbe => {
                pop_n(&mut stack, 1);
                stack.push(ScanVal::Unknown);
            }
            0xbf => {
                pop_n(&mut stack, 1);
            }
            0xc0 | 0xc1 => {
                pop_n(&mut stack, 1);
                stack.push(ScanVal::Unknown);
            }
            0xc2 | 0xc3 => {
                pop_n(&mut stack, 1);
            }
            0xc5 => {
                pop_n(&mut stack, 1);
                stack.push(ScanVal::Unknown);
            }
            0xc6 | 0xc7 => {
                pop_n(&mut stack, 1);
            }
            _ => {}
        }
    }
    // Resolve ofLegacyCopy sources.
    for _ in 0..8 {
        for index in 0..values.len() {
            if let Some(source) = values[index].copy {
                if source != index {
                    let source_overrides = values[source].clone();
                    let entry = &mut values[index];
                    if entry.spawn.is_none() {
                        entry.spawn = source_overrides.spawn;
                    }
                    if entry.redstone.is_none() {
                        entry.redstone = source_overrides.redstone;
                    }
                    if entry.light.is_none() {
                        entry.light = source_overrides.light;
                    }
                    if entry.instrument.is_none() {
                        entry.instrument = source_overrides.instrument.clone();
                    }
                    if entry.air.is_none() {
                        entry.air = source_overrides.air;
                    }
                    if entry.suffocating.is_none() {
                        entry.suffocating = source_overrides.suffocating;
                    }
                }
            }
        }
    }
    let _ = field_to_index;
    let _ = last_blocks_field;
    Ok(Scan { blocks: values, notes })
}

#[derive(Clone, Debug)]
enum ScanVal {
    Int(i32),
    Props(Overrides),
    Func(String),
    Light(LightKind),
    Text(String),
    BlockRef { id: Option<String> },
    Unknown,
}

fn pop_n(stack: &mut Vec<ScanVal>, count: usize) {
    for _ in 0..count {
        stack.pop();
    }
}

fn returns_value(descriptor: &str) -> bool {
    classfile::descriptor_types(descriptor)
        .last()
        .map(|kind| kind != "V")
        .unwrap_or(false)
}

fn returns_block(descriptor: &str) -> bool {
    classfile::descriptor_types(descriptor)
        .last()
        .map(|kind| kind.starts_with("Lnet/minecraft/world/level/block/Block;"))
        .unwrap_or(false)
}

fn pop_args(stack: &mut Vec<ScanVal>, descriptor: &str) -> Vec<ScanVal> {
    let count = classfile::descriptor_types(descriptor)
        .len()
        .saturating_sub(1);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(stack.pop().unwrap_or(ScanVal::Unknown));
    }
    out.reverse();
    out
}

fn block_item_ids(class: &ClassFile) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(code) = class
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
        .map(|code| code.bytes.clone())
    else {
        return out;
    };
    let Ok(ins) = decode(&code) else { return out };
    let mut pending: Option<String> = None;
    for current in &ins {
        match current.op {
            0x12 | 0x13 => {
                if let Some(text) =
                    classfile::string_constant(&class.constants, current.operand as usize)
                {
                    pending = Some(text);
                }
            }
            0xb3 => {
                if let Some((owner, name, _)) =
                    classfile::field_ref(&class.constants, current.operand as usize)
                {
                    if owner.ends_with("BlockItemIds") {
                        if let Some(id) = pending.take() {
                            out.insert(name, id);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn classify_spawn(name: &str) -> SpawnKind {
    match name {
        "never" => SpawnKind::Never,
        "always" => SpawnKind::Always,
        "ocelotOrParrot" => SpawnKind::OcelotOrParrot,
        "lambda$static$146" | "lambda$static$287" => SpawnKind::PolarBear,
        "lambda$static$289" => SpawnKind::FireImmune,
        _ => SpawnKind::Default,
    }
}

fn classify_redstone(name: &str) -> RedstoneKind {
    match name {
        "never" => RedstoneKind::Never,
        "always" => RedstoneKind::Always,
        _ => RedstoneKind::Default,
    }
}

/// `Properties.isSuffocating` predicates in `Blocks.<clinit>`: the two
/// constants, else the default `causes_suffocation && collision-full` rule.
fn classify_predicate(class: &ClassFile, name: &str) -> RedstoneKind {
    let Some(code) = class
        .method(name)
        .and_then(|method| method.code.as_ref())
        .map(|code| code.bytes.clone())
    else {
        return RedstoneKind::Default;
    };
    let Ok(ins) = decode(&code) else {
        return RedstoneKind::Default;
    };
    let ops: Vec<u8> = ins.iter().map(|ins| ins.op).collect();
    if ops.iter().any(|op| *op == 0xac || *op == 0xad || *op == 0xae || *op == 0xaf) {
        // A constant-returning predicate: the last `iconst_*` before `ireturn`.
        if let Some(return_at) = ops.iter().rposition(|op| *op == 0xac) {
            if return_at > 0 {
                match ops[return_at - 1] {
                    0x03 => return RedstoneKind::Always,
                    0x02 => return RedstoneKind::Never,
                    _ => {}
                }
            }
        }
    }
    RedstoneKind::Default
}

fn classify_light(class: &ClassFile, name: &str) -> LightKind {
    let Some(code) = class
        .method(name)
        .and_then(|method| method.code.as_ref())
        .map(|code| code.bytes.clone())
    else {
        return LightKind::Unknown;
    };
    let Ok(ins) = decode(&code) else {
        return LightKind::Unknown;
    };
    let mut references = Vec::new();
    let mut last_int = None;
    for current in &ins {
        match current.op {
            0x02..=0x08 => last_int = Some(current.op as i32 - 3),
            0x10 | 0x11 => last_int = Some(current.operand),
            0xb2 => {
                if let Some((owner, member, _)) =
                    classfile::field_ref(&class.constants, current.operand as usize)
                {
                    references.push(format!("{owner}.{member}"));
                }
            }
            0xb6 | 0xb8 => {
                if let Some((owner, member, _)) =
                    classfile::method_ref(&class.constants, current.operand as usize)
                {
                    references.push(format!("{owner}.{member}"));
                }
            }
            _ => {}
        }
    }
    let joined = references.join(",");
    if joined.contains("VaultBlock.STATE") || joined.contains("VaultState") {
        LightKind::Vault
    } else if joined.contains("TrialSpawner") {
        LightKind::TrialSpawner
    } else if joined.contains("RespawnAnchorBlock.getScaledChargeLevel") {
        LightKind::Max(15)
    } else if joined.contains("SeaPickleBlock.PICKLES") {
        LightKind::SeaPickle
    } else if let Some(value) = last_int {
        LightKind::Constant(value)
    } else {
        LightKind::Unknown
    }
}

fn literal_int(class: &ClassFile, name: &str) -> Option<i32> {
    let code = class
        .method(name)
        .and_then(|method| method.code.as_ref())
        .map(|code| code.bytes.clone())?;
    let ins = decode(&code).ok()?;
    let mut value = None;
    for current in &ins {
        match current.op {
            0x02..=0x08 => value = Some(current.op as i32 - 3),
            0x10 | 0x11 => value = Some(current.operand),
            _ => {}
        }
    }
    value
}

fn indy_target(class: &ClassFile, index: usize) -> Option<String> {
    let classfile::Constant::InvokeDynamic { bootstrap, .. } = class.constants.get(index)? else {
        return None;
    };
    let entry = class.bootstraps.get(*bootstrap)?;
    for argument in &entry.arguments {
        let Some(classfile::Constant::MethodHandle { reference }) = class.constants.get(*argument)
        else {
            continue;
        };
        let (owner, name) = match class.constants.get(*reference) {
            Some(classfile::Constant::Methodref { class: owner, name, .. })
            | Some(classfile::Constant::InterfaceMethodref { class: owner, name, .. }) => {
                (owner.clone(), name.clone())
            }
            _ => continue,
        };
        let owner_at: usize = owner.trim_start_matches('#').parse().ok()?;
        let name_at: usize = name.trim_start_matches('#').parse().ok()?;
        let _owner = match class.constants.get(owner_at) {
            Some(classfile::Constant::Class(reference)) => {
                let inner: usize = reference.trim_start_matches('#').parse().ok()?;
                classfile::utf8(&class.constants, inner)
            }
            _ => String::new(),
        };
        let method = match class.constants.get(name_at) {
            Some(classfile::Constant::NameAndType { name, .. }) => {
                let inner: usize = name.trim_start_matches('#').parse().ok()?;
                classfile::utf8(&class.constants, inner)
            }
            _ => continue,
        };
        return Some(method);
    }
    None
}

fn indy_descriptor(class: &ClassFile, index: usize) -> String {
    let Some(classfile::Constant::InvokeDynamic { name, .. }) = class.constants.get(index) else {
        return String::new();
    };
    let at: usize = name.trim_start_matches('#').parse().unwrap_or(0);
    match class.constants.get(at) {
        Some(classfile::Constant::NameAndType { kind, .. }) => {
            let inner: usize = kind.trim_start_matches('#').parse().unwrap_or(0);
            classfile::utf8(&class.constants, inner)
        }
        _ => String::new(),
    }
}

fn lower(name: &str) -> String {
    name.to_ascii_lowercase()
}

// ---------------------------------------------------------------------------
// a tiny bytecode decoder (branches resolved, switches skipped)
// ---------------------------------------------------------------------------

struct Ins {
    op: u8,
    operand: i32,
}

fn op_length(op: u8) -> usize {
    match op {
        0x10 | 0x12 | 0x15 | 0x16 | 0x17 | 0x18 | 0x19 | 0x36 | 0x37 | 0x38 | 0x39 | 0x3a | 0xa9
        | 0xbc => 2,
        0x11 | 0x13 | 0x14 | 0x84 | 0xb2 | 0xb3 | 0xb4 | 0xb5 | 0xb6 | 0xb7 | 0xb8 | 0xbb
        | 0xbd | 0xc0 | 0xc1 | 0xc6 | 0xc7 => 3,
        0x99..=0xa8 => 3,
        0xb9 | 0xba | 0xc8 | 0xc9 => 5,
        0xc5 => 4,
        _ => 1,
    }
}

fn decode(code: &[u8]) -> Result<Vec<Ins>, String> {
    let mut out = Vec::with_capacity(code.len() / 2);
    let mut at = 0usize;
    while at < code.len() {
        let op = code[at];
        if op == 0xaa || op == 0xab {
            // tableswitch / lookupswitch: skip the padding and the table.
            let padded = (at + 4) & !3;
            let mut position = padded;
            let length = if op == 0xaa {
                let low = i32::from_be_bytes([
                    code[position + 4],
                    code[position + 5],
                    code[position + 6],
                    code[position + 7],
                ]);
                let high = i32::from_be_bytes([
                    code[position + 8],
                    code[position + 9],
                    code[position + 10],
                    code[position + 11],
                ]);
                position += 12;
                let count = (high - low + 1).max(0) as usize;
                at = position + 4 * count;
                out.push(Ins { op, operand: 0 });
                continue;
            } else {
                let count = i32::from_be_bytes([
                    code[position + 4],
                    code[position + 5],
                    code[position + 6],
                    code[position + 7],
                ]) as usize;
                at = position + 8 + 8 * count;
                out.push(Ins { op, operand: 0 });
                continue;
            };
            let _ = length;
        }
        let length = op_length(op);
        let mut operand = 0i32;
        match op {
            0x10 => operand = code.get(at + 1).copied().unwrap_or(0) as i8 as i32,
            0x11 => operand = i16::from_be_bytes([code[at + 1], code[at + 2]]) as i32,
            0x12 | 0x15..=0x19 | 0x36..=0x3a | 0xbc => {
                operand = code.get(at + 1).copied().unwrap_or(0) as i32
            }
            0x84 => operand = code.get(at + 1).copied().unwrap_or(0) as i32,
            0x99..=0xa8 | 0xc6 | 0xc7 => {
                operand = i16::from_be_bytes([code[at + 1], code[at + 2]]) as i32
            }
            0xb2..=0xb8 | 0xbb | 0xbd | 0xc0 | 0xc1 | 0xb9 | 0xba | 0x13 | 0x14 | 0xc5 => {
                operand = u16::from_be_bytes([code[at + 1], code[at + 2]]) as i32
            }
            _ => {}
        }
        out.push(Ins { op, operand });
        at += length;
    }
    Ok(out)
}

/// Stack effect of an instruction (pops, pushes), values counted one per slot.
fn stack_effect(op: u8) -> (usize, usize) {
    match op {
        0x00 => (0, 0),
        0x01..=0x14 => (0, 1),
        0x15..=0x2d => (0, 1),
        0x2e..=0x35 => (2, 1),
        0x36..=0x4e => (1, 0),
        0x4f..=0x56 => (3, 0),
        0x57 => (1, 0),
        0x58 => (1, 0),
        0x59 => (1, 2),
        0x5a => (2, 3),
        0x5b => (3, 4),
        0x5c => (2, 4),
        0x5d => (3, 5),
        0x5e => (4, 6),
        0x5f => (2, 2),
        0x60..=0x8f => (2, 1),
        0x90..=0x93 => (1, 1),
        0x94..=0x98 => (2, 1),
        0x99..=0xa6 => (1, 0),
        0xa7 | 0xc8 | 0xc9 => (0, 0),
        0xa8 => (0, 1),
        0xa9 => (0, 0),
        0xac..=0xb1 => (0, 0),
        0xb2 => (0, 1),
        0xb3 => (1, 0),
        0xb4 => (1, 1),
        0xb5 => (2, 0),
        0xbb => (0, 1),
        0xbc | 0xbd => (1, 1),
        0xbe => (1, 1),
        0xbf => (1, 0),
        0xc0 | 0xc1 => (1, 1),
        0xc2 | 0xc3 => (1, 0),
        0xc5 => (1, 1),
        0xc6 | 0xc7 => (1, 0),
        _ => (0, 0),
    }

}
// ---------------------------------------------------------------------------
// the table
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct MobStates {
    /// `[pathType, flags, height]` per registry state.
    pub rows: Vec<(String, i32, f64)>,
    pub gaps: Vec<String>,
    pub notes: Vec<String>,
}

impl MobStates {
    pub fn to_value(&self) -> Value {
        Value::Array(
            self.rows
                .iter()
                .map(|(path, flags, height)| {
                    Value::Array(vec![
                        Value::String(path.clone()),
                        Value::Number(serde_json::Number::from(*flags)),
                        Value::Number(
                            serde_json::Number::from_f64(*height)
                                .unwrap_or_else(|| serde_json::Number::from(0)),
                        ),
                    ])
                })
                .collect(),
        )
    }
}

/// Debug helper: the scanned property overrides for one block.
pub fn debug_overrides(jar_path: &Path, id: &str) -> Result<String, String> {
    let registry = blockmod::registry(jar_path)?;
    let scan = scan_properties(jar_path, &registry)?;
    let mut index = None;
    for (block_index, block) in registry.iter().enumerate() {
        if block.id == id || block.id == format!("minecraft:{id}") {
            index = Some(block_index);
        }
    }
    let Some(index) = index else { return Err("not found".to_string()) };
    Ok(format!("{} -> {:?}", registry[index].id, scan.blocks[index]))
}
/// Debug helper: the support shape's coordinates/occupancy for one block.
pub fn debug_shape(jar_path: &Path, block_id: &str) -> Result<String, String> {
    let evaluated = shape_final::evaluate(jar_path)?;
    let table = statemod::state_table(jar_path)?;
    let registry = blockmod::registry(jar_path)?;
    let mut index = None;
    let mut offset = 0usize;
    for (block_index, states) in table.blocks.iter().enumerate() {
        if block_id == "minecraft:first" {
            index = Some((block_index, offset));
            break;
        }
        let full = format!("minecraft:{}", block_id);
        if states.id == full || states.id == block_id {
            index = Some((block_index, offset));
            break;
        }
        offset += states.state_count;
    }
    let Some((block_index, offset)) = index else {
        return Err(format!("{block_id} not found"));
    };
    let support = evaluated.support[offset].clone();
    let collision = evaluated.collision[offset].clone();
    Ok(format!(
        "{} idx={} states={} x={:?} y={:?} z={:?} occ={:?} dmin={:?} dmax={:?} canon={:?} cube={:?} axes={:?}\ncollision x={:?} y={:?} z={:?}",
        registry[block_index].id,
        block_index,
        table.blocks[block_index].state_count,
        support.as_ref().map(|s| s.x.clone()).unwrap_or_default(),
        support.as_ref().map(|s| s.y.clone()).unwrap_or_default(),
        support.as_ref().map(|s| s.z.clone()).unwrap_or_default(),
        support.as_ref().map(|s| s.occupied.clone()).unwrap_or_default(),
        support.as_ref().map(|s| s.discrete_min).unwrap_or_default(),
        support.as_ref().map(|s| s.discrete_max).unwrap_or_default(),
        support.as_ref().map(|s| s.canonical),
        support.as_ref().map(|s| s.cube),
        support.as_ref().map(|s| s.cube_axes),
        collision.as_ref().map(|s| s.x.clone()).unwrap_or_default(),
        collision.as_ref().map(|s| s.y.clone()).unwrap_or_default(),
        collision.as_ref().map(|s| s.z.clone()).unwrap_or_default(),
    ))
}
pub fn mob_states(jar_path: &Path) -> Result<MobStates, String> {
    let evaluated = shape_final::evaluate(jar_path)?;
    let registry = blockmod::registry(jar_path)?;
    let table = statemod::state_table(jar_path)?;
    let properties = shape_props::tables(jar_path)?;
    let has_collision = shape_flags::has_collision(jar_path)?;
    let tags = Tags::load(jar_path)?;
    let scan = scan_properties(jar_path, &registry)?;
    if registry.len() != table.blocks.len() {
        return Err("block registry and state table disagree".to_string());
    }
    let mut rows = Vec::with_capacity(table.state_count());
    let mut gaps = Vec::new();
    let mut notes = scan.notes;
    for (block_index, states) in table.blocks.iter().enumerate() {
        let block = &registry[block_index];
        let overrides = scan
            .blocks
            .get(block_index)
            .cloned()
            .unwrap_or_default();
        let force_solid_on = properties
            .blocks
            .get(block_index)
            .map(|(_, props)| props.force_solid_on)
            .unwrap_or(false);
        let force_solid_off = properties
            .blocks
            .get(block_index)
            .map(|(_, props)| props.force_solid_off)
            .unwrap_or(false);
        let collision_enabled = has_collision.get(block_index).copied().unwrap_or(true);
        let behaviour = pathfindable_behavior(&block.chain);
        let signal = is_signal_source(&block.chain);
        let can_open_by_hand = block.id != "minecraft:iron_door";
        for offset in 0..states.state_count {
            let state_index = rows.len();
            let values = state_values(states, offset);
            let fluid = fluid_state(&block.chain, &block.id, &values);
            let collision = evaluated.collision.get(state_index).cloned().flatten();
            let occlusion = evaluated.occlusion.get(state_index).cloned().flatten();
            let support = evaluated.support.get(state_index).cloned().flatten();
            let collision_full = collision
                .as_ref()
                .map(shape_is_full)
                .unwrap_or(false);
            let solid_render = occlusion.as_ref().map(shape_is_full).unwrap_or(false);
            let face_sturdy_up = support.as_ref().map(shape_face_full_up).unwrap_or(false);
            let light = light_emission(overrides.light, &values);
            let mut flags = 0i32;
            if pathfindable(
                behaviour,
                &values,
                fluid,
                collision_full,
                collision_enabled,
                true,
            ) {
                flags |= 1;
            }
            if pathfindable(
                behaviour,
                &values,
                fluid,
                collision_full,
                collision_enabled,
                false,
            ) {
                flags |= 2;
            }
            if pathfindable(
                behaviour,
                &values,
                fluid,
                collision_full,
                collision_enabled,
                true,
            ) && behaviour == Pathfindable::Default
            {
                flags |= 4;
            } else if air_pathfindable(behaviour, &values, collision_full, collision_enabled) {
                flags |= 4;
            }
            let default_spawn = face_sturdy_up && light < 14;
            for (bit, spawn_kind) in [
                (8, SpawnKind::from(overrides.spawn)),
                (16, SpawnKind::from(overrides.spawn)),
                (32, SpawnKind::from(overrides.spawn)),
            ] {
                let valid = match spawn_kind {
                    SpawnKind::Default => default_spawn,
                    SpawnKind::Never => false,
                    SpawnKind::Always => true,
                    SpawnKind::OcelotOrParrot => bit == 16,
                    SpawnKind::PolarBear => bit == 32,
                    SpawnKind::FireImmune => false,
                };
                if valid {
                    flags |= bit;
                }
            }
            let redstone = match RedstoneKind::from(overrides.redstone) {
                RedstoneKind::Default => {
                    tags.has("causes_suffocation", &block.id) && collision_full
                }
                RedstoneKind::Never => false,
                RedstoneKind::Always => true,
            };
            if redstone {
                flags |= 64;
            }
            if !collision_full
                && !signal
                && fluid == Fluid::None
                && !tags.has("prevent_mob_spawning_inside", &block.id)
                && !zombie_dangerous(&tags, &block.id, &values)
            {
                flags |= 128;
            }
            if face_sturdy_up {
                flags |= 256;
            }
            let solid = if force_solid_on {
                true
            } else if force_solid_off {
                false
            } else if !collision_enabled || collision.is_none() {
                false
            } else if let Some(shape) = collision.as_ref() {
                if shape_is_empty(shape) {
                    false
                } else {
                    let bounds = shape_bounds(shape);
                    let volume = (bounds[3] - bounds[0]) * (bounds[4] - bounds[1]) * (bounds[5] - bounds[2]);
                    volume >= 0.7291666666666666 || (bounds[4] - bounds[1]) >= 1.0
                }
            } else {
                false
            };
            if solid {
                flags |= 512;
            }
            if solid_render {
                flags |= 1024;
            }
            let path = path_type(
                &block.id,
                &block.chain,
                &values,
                fluid,
                collision_full,
                collision_enabled,
                can_open_by_hand,
                &tags,
            );
            let height = shape_height(&collision);
            if collision.is_none() {
                gaps.push(format!("{}: dynamic collision shape", block.id));
            }
            rows.push((path.to_string(), flags, height));
        }
    }
    Ok(MobStates { rows, gaps, notes })
}

impl From<Option<SpawnKind>> for SpawnKind {
    fn from(value: Option<SpawnKind>) -> Self {
        value.unwrap_or(SpawnKind::Default)
    }
}

impl From<Option<RedstoneKind>> for RedstoneKind {
    fn from(value: Option<RedstoneKind>) -> Self {
        value.unwrap_or(RedstoneKind::Default)
    }
}

fn light_emission(kind: Option<LightKind>, values: &BTreeMap<String, String>) -> i32 {
    match kind.unwrap_or(LightKind::Zero) {
        LightKind::Zero | LightKind::Unknown => 0,
        LightKind::Constant(value) => value,
        LightKind::Lit(value) => {
            if property_true(values, "lit") {
                value
            } else {
                0
            }
        }
        LightKind::Max(value) => {
            let charge = property_int(values, "charge").unwrap_or(0);
            // `RespawnAnchorBlock.getScaledChargeLevel`: charge * 15 / 4.
            charge.min(4) * value / 4
        }
        LightKind::SeaPickle => {
            let pickles = property_int(values, "pickles").unwrap_or(1);
            if pickles == 4 {
                12
            } else {
                pickles * 3
            }
        }
        LightKind::Vault => 0,
        LightKind::TrialSpawner => 0,
    }
}

fn pathfindable(
    behaviour: Pathfindable,
    values: &BTreeMap<String, String>,
    fluid: Fluid,
    collision_full: bool,
    collision_enabled: bool,
    land: bool,
) -> bool {
    match behaviour {
        Pathfindable::Default => {
            if land {
                !collision_full
            } else {
                fluid == Fluid::Water
            }
        }
        Pathfindable::Never => false,
        Pathfindable::Door => property_true(values, "open"),
        Pathfindable::TrapDoor => {
            if land {
                property_true(values, "open")
            } else {
                property_true(values, "waterlogged")
            }
        }
        Pathfindable::FenceGate => property_true(values, "open"),
        Pathfindable::Slab => {
            if land {
                false
            } else {
                fluid == Fluid::Water
            }
        }
        Pathfindable::SnowLayer => {
            if land {
                property_int(values, "layers").unwrap_or(1) < 5
            } else {
                false
            }
        }
        Pathfindable::Liquid => fluid != Fluid::Lava,
        Pathfindable::WaterOnly => fluid == Fluid::Water,
        Pathfindable::Vegetation => {
            if land {
                !collision_full
            } else if !collision_enabled {
                true
            } else {
                !collision_full
            }
        }
    }
}

fn air_pathfindable(
    behaviour: Pathfindable,
    values: &BTreeMap<String, String>,
    collision_full: bool,
    collision_enabled: bool,
) -> bool {
    match behaviour {
        Pathfindable::Never | Pathfindable::Slab | Pathfindable::SnowLayer => false,
        Pathfindable::Door | Pathfindable::TrapDoor | Pathfindable::FenceGate => {
            property_true(values, "open")
        }
        Pathfindable::Liquid => values.get("level").is_some(),
        Pathfindable::WaterOnly => false,
        Pathfindable::Vegetation => !collision_enabled || !collision_full,
        Pathfindable::Default => !collision_full,
    }
}

fn zombie_dangerous(
    tags: &Tags,
    block_id: &str,
    values: &BTreeMap<String, String>,
) -> bool {
    if tags.has("default_immune_to", block_id) {
        return false;
    }
    if is_burning_block(tags, block_id, values) {
        return true;
    }
    matches!(
        block_id,
        "minecraft:wither_rose"
            | "minecraft:sweet_berry_bush"
            | "minecraft:cactus"
            | "minecraft:powder_snow"
    )
}

fn is_burning_block(tags: &Tags, block_id: &str, values: &BTreeMap<String, String>) -> bool {
    tags.has("fire", block_id)
        || block_id == "minecraft:lava"
        || block_id == "minecraft:magma_block"
        || is_lit_campfire(tags, block_id, values)
        || block_id == "minecraft:lava_cauldron"
}

fn is_lit_campfire(tags: &Tags, block_id: &str, values: &BTreeMap<String, String>) -> bool {
    values.contains_key("lit") && tags.has("campfires", block_id) && property_true(values, "lit")
}

fn path_type(
    block_id: &str,
    chain: &[String],
    values: &BTreeMap<String, String>,
    fluid: Fluid,
    collision_full: bool,
    collision_enabled: bool,
    can_open_by_hand: bool,
    tags: &Tags,
) -> &'static str {
    let _ = collision_enabled;
    if block_id == "minecraft:air"
        || block_id == "minecraft:cave_air"
        || block_id == "minecraft:void_air"
    {
        return "OPEN";
    }
    if tags.has("trapdoors", block_id)
        || block_id == "minecraft:lily_pad"
        || block_id == "minecraft:big_dripleaf"
    {
        return "TRAPDOOR";
    }
    if block_id == "minecraft:powder_snow" {
        return "POWDER_SNOW";
    }
    if block_id == "minecraft:cactus" || block_id == "minecraft:sweet_berry_bush" {
        return "DAMAGING";
    }
    if block_id == "minecraft:honey_block" {
        return "STICKY_HONEY";
    }
    if block_id == "minecraft:cocoa" {
        return "COCOA";
    }
    if block_id == "minecraft:wither_rose" || tags.has("speleothems", block_id) {
        return "DAMAGE_CAUTIOUS";
    }
    if fluid == Fluid::Lava {
        return "LAVA";
    }
    if tags.has("fire", block_id)
        || block_id == "minecraft:lava"
        || block_id == "minecraft:magma_block"
        || is_lit_campfire(tags, block_id, values)
        || block_id == "minecraft:lava_cauldron"
    {
        return "FIRE";
    }
    if chain.iter().any(|class| class == "DoorBlock") {
        if property_true(values, "open") {
            return "DOOR_OPEN";
        }
        return if can_open_by_hand {
            "DOOR_WOOD_CLOSED"
        } else {
            "DOOR_IRON_CLOSED"
        };
    }
    if chain.iter().any(|class| class == "BaseRailBlock") {
        return "RAIL";
    }
    if chain.iter().any(|class| class == "LeavesBlock") {
        return "LEAVES";
    }
    let fence_gate_closed = chain.iter().any(|class| class == "FenceGateBlock")
        && !property_true(values, "open");
    if tags.has("fences", block_id) || tags.has("walls", block_id) || fence_gate_closed {
        return "FENCE";
    }
    let behaviour = pathfindable_behavior(chain);
    if !pathfindable(behaviour, values, fluid, collision_full, true, true) {
        return "BLOCKED";
    }
    if fluid == Fluid::Water {
        return "WATER";
    }
    "OPEN"
}

/// Write `mob-data.json#states` (compact array).
pub fn write_states(jar_path: &Path, out: impl AsRef<Path>) -> Result<MobStates, String> {
    let state = mob_states(jar_path)?;
    let text = serde_json::to_string(&state.to_value()).map_err(|e| e.to_string())?;
    let out = out.as_ref();
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(out, text).map_err(|e| format!("{}: {e}", out.display()))?;
    Ok(state)
}

// ===========================================================================
// block-behaviours.json#states
// ===========================================================================

/// The states table this module reproduces.
pub const BEHAVIOUR_TABLE: &str = "block-behaviours.json";
pub const BEHAVIOUR_DATA_VERSION: i32 = 5023;

/// One `states` row, in the exporter's column order.
#[derive(Debug, Clone, PartialEq)]
pub struct StateRow {
    pub sturdy: i64,
    pub flags: i32,
    pub fluid: i32,
    pub fluid_amount: i32,
    pub falling: bool,
    pub weak: i64,
    pub strong: i64,
    pub signal_error: bool,
    pub instrument: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BehaviourStates {
    pub rows: Vec<StateRow>,
    pub gaps: Vec<String>,
    pub notes: Vec<String>,
}

impl BehaviourStates {
    pub fn to_value(&self) -> Value {
        Value::Array(
            self.rows
                .iter()
                .map(|row| {
                    Value::Array(vec![
                        Value::Number(row.sturdy.into()),
                        Value::Number(row.flags.into()),
                        Value::Number(row.fluid.into()),
                        Value::Number(row.fluid_amount.into()),
                        Value::Bool(row.falling),
                        Value::Number(row.weak.into()),
                        Value::Number(row.strong.into()),
                        Value::Bool(row.signal_error),
                        Value::String(row.instrument.clone()),
                    ])
                })
                .collect(),
        )
    }
}

pub fn behaviour_states(jar_path: &Path) -> Result<BehaviourStates, String> {
    let dbg = std::env::var_os("BEHAVIOUR_DEBUG").is_some();
    macro_rules! step {
        ($label:expr) => {
            if dbg {
                eprintln!("[behaviour] {}", $label);
            }
        };
    }
    step!("evaluate");
    let evaluated = shape_final::evaluate(jar_path)?;
    step!("registry");
    let registry = blockmod::registry(jar_path)?;
    let table = statemod::state_table(jar_path)?;
    step!("properties");
    let properties = shape_props::tables(jar_path)?;
    let has_collision = shape_flags::has_collision(jar_path)?;
    step!("scan");
    let scan = scan_properties(jar_path, &registry)?;
    step!("fluid ids");
    let fluid_ids = fluid_ids(jar_path)?;
    step!("class flags");
    let class_flags = class_flags(jar_path)?;
    step!("rows loop");
    if registry.len() != table.blocks.len() {
        return Err("block registry and state table disagree".to_string());
    }
    let center_shape = shape_from_boxes(&[(7.0, 0.0, 7.0, 9.0, 10.0, 9.0)]);
    let rigid_shape = shape_from_boxes(&[
        (0.0, 0.0, 0.0, 2.0, 16.0, 16.0),
        (14.0, 0.0, 0.0, 16.0, 16.0, 16.0),
        (2.0, 0.0, 0.0, 14.0, 16.0, 2.0),
        (2.0, 0.0, 14.0, 14.0, 16.0, 16.0),
    ]);

    let mut rows = Vec::with_capacity(table.state_count());
    let mut gaps = Vec::new();
    let mut notes = scan.notes;
    let mut state_index = 0usize;
    for (block_index, states) in table.blocks.iter().enumerate() {
        let block = &registry[block_index];
        let overrides = scan
            .blocks
            .get(block_index)
            .cloned()
            .unwrap_or_default();
        let props = properties.blocks.get(block_index).map(|(_, props)| props);
        let force_solid_on = props.map(|props| props.force_solid_on).unwrap_or(false);
        let force_solid_off = props.map(|props| props.force_solid_off).unwrap_or(false);
        let collision_enabled = has_collision.get(block_index).copied().unwrap_or(true);
        let signal_source = class_flags.signal_source(&block.chain);
        let analog = class_flags.analog(&block.chain);
        let instrument_default = overrides
            .instrument
            .clone()
            .unwrap_or_else(|| "HARP".to_string());
        let air = overrides.air.unwrap_or(false);
        for offset in 0..states.state_count {
            let values = state_values(states, offset);
            let collision = evaluated.collision.get(state_index).cloned().flatten();
            let occlusion = evaluated.occlusion.get(state_index).cloned().flatten();
            let support = evaluated.support.get(state_index).cloned().flatten();
            let collision_full = collision.as_ref().map(shape_is_full).unwrap_or(false);
            if collision.is_none() || support.is_none() {
                gaps.push(format!("{}: dynamic shape", block.id));
            }
            let mut flags = 0i32;
            let solid = if force_solid_on {
                true
            } else if force_solid_off {
                false
            } else if !collision_enabled {
                false
            } else if let Some(shape) = collision.as_ref() {
                if shape_is_empty(shape) {
                    false
                } else {
                    let bounds = shape_bounds(shape);
                    let volume = (bounds[3] - bounds[0])
                        * (bounds[4] - bounds[1])
                        * (bounds[5] - bounds[2]);
                    volume >= 0.7291666666666666 || (bounds[4] - bounds[1]) >= 1.0
                }
            } else {
                false
            };
            if solid {
                flags |= 1;
            }
            if air {
                flags |= 2;
            }
            let redstone = match RedstoneKind::from(overrides.redstone) {
                RedstoneKind::Default => collision_full,
                RedstoneKind::Never => false,
                RedstoneKind::Always => true,
            };
            if redstone {
                flags |= 4;
            }
            if signal_source {
                flags |= 8;
            }
            if analog {
                flags |= 16;
            }
            let collision_empty = collision.as_ref().map(shape_is_empty).unwrap_or(false);
            if collision_empty {
                flags |= 32;
            }
            let suffocating = suffocating_rule(&block.chain, &values);
            if suffocating {
                flags |= 64;
            }
            let sturdy = support
                .as_ref()
                .map(|shape| sturdy_mask(shape, &center_shape, &rigid_shape))
                .unwrap_or(0);
            let (fluid, fluid_amount, falling) =
                fluid_row(&block.chain, &block.id, &values, &fluid_ids);
            let (weak, strong) = if signal_source {
                signal_masks(&block.chain, &values)
            } else {
                (0, 0)
            };
            let _ = occlusion;
            rows.push(StateRow {
                sturdy,
                flags,
                fluid,
                fluid_amount,
                falling,
                weak,
                strong,
                signal_error: false,
                instrument: instrument_default.clone(),
            });
            state_index += 1;
        }
    }
    step!("rows");
    Ok(BehaviourStates { rows, gaps, notes })
}

/// The fluid registry order (`Fluids.<clinit>` field order).
fn fluid_ids(jar_path: &Path) -> Result<BTreeMap<String, i32>, String> {
    let file = fs::File::open(jar_path).map_err(|e| format!("{}: {e}", jar_path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", jar_path.display()))?;
    let mut entry = archive
        .by_name("net/minecraft/world/level/material/Fluids.class")
        .map_err(|e| format!("Fluids: {e}"))?;
    let mut data = Vec::new();
    use std::io::Read;
    entry.read_to_end(&mut data).map_err(|e| e.to_string())?;
    let class = classfile::parse(&data).map_err(|e| format!("Fluids: {e}"))?;
    let mut out = BTreeMap::new();
    for field in &class.fields {
        if field.descriptor.starts_with("Lnet/minecraft/world/level/material/") {
            let id = out.len() as i32;
            out.insert(field.name.to_lowercase(), id);
        }
    }
    Ok(out)
}

fn fluid_row(
    chain: &[String],
    block_id: &str,
    values: &BTreeMap<String, String>,
    ids: &BTreeMap<String, i32>,
) -> (i32, i32, bool) {
    let empty = *ids.get("empty").unwrap_or(&0);
    if chain.iter().any(|class| class == "LiquidBlock") {
        let level = property_int(values, "level").unwrap_or(0);
        if block_id == "minecraft:lava" {
            if level == 0 {
                return (*ids.get("lava").unwrap_or(&4), 8, false);
            }
            let amount = if level <= 7 { 8 - level } else { 8 };
            return (*ids.get("flowing_lava").unwrap_or(&3), amount, level >= 8);
        }
        if block_id == "minecraft:water" {
            let amount = if level <= 7 { 8 - level } else { 8 };
            return (*ids.get("flowing_water").unwrap_or(&1), amount, level >= 8);
        }
    }
    if property_true(values, "waterlogged") {
        let falling = chain.iter().any(|class| {
            class == "WeatheringCopperGrateBlock" || class == "WaterloggedTransparentBlock"
        });
        return (*ids.get("water").unwrap_or(&2), 8, falling);
    }
    let water_source = [
        "SeagrassBlock",
        "TallSeagrassBlock",
        "KelpBlock",
        "KelpPlantBlock",
        "BubbleColumnBlock",
    ];
    if chain.iter().any(|class| water_source.contains(&class.as_str())) {
        return (*ids.get("water").unwrap_or(&2), 8, false);
    }
    (empty, 0, false)
}

/// Per-class overrides for the state facts that are not `Properties` fields.
#[derive(Debug, Default, Clone)]
struct ClassFlags {
    signal_classes: BTreeSet<String>,
    analog_classes: BTreeSet<String>,
}

impl ClassFlags {
    fn signal_source(&self, chain: &[String]) -> bool {
        chain.iter().any(|class| self.signal_classes.contains(class))
    }

    fn analog(&self, chain: &[String]) -> bool {
        chain.iter().any(|class| self.analog_classes.contains(class))
    }
}

/// Scan every block class for `isSignalSource`/`hasAnalogOutputSignal`
/// declarations, keeping the ones that return a constant.
fn class_flags(jar_path: &Path) -> Result<ClassFlags, String> {
    let file = fs::File::open(jar_path).map_err(|e| format!("{}: {e}", jar_path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", jar_path.display()))?;
    let mut flags = ClassFlags::default();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().map_err(|e| e.to_string())?.to_string();
        let Some(internal) = name
            .strip_prefix("net/minecraft/world/level/block/")
            .and_then(|rest| rest.strip_suffix(".class"))
        else {
            continue;
        };
        if internal.contains('$') {
            continue;
        }
        let mut data = Vec::new();
        use std::io::Read;
        entry.read_to_end(&mut data).map_err(|e| e.to_string())?;
        let Ok(class) = classfile::parse(&data) else {
            continue;
        };
        let simple = internal.rsplit('/').next().unwrap_or(internal).to_string();
        for method in &class.methods {
            let constant_true = method
                .code
                .as_ref()
                .map(|code| constant_bool(&code.bytes) == Some(true))
                .unwrap_or(false);
            if method.name == "isSignalSource"
                && method.descriptor == "(Lnet/minecraft/world/level/block/state/BlockState;)Z"
                && constant_true
            {
                flags.signal_classes.insert(simple.clone());
            }
            if method.name == "hasAnalogOutputSignal"
                && method.descriptor == "(Lnet/minecraft/world/level/block/state/BlockState;)Z"
                && constant_true
            {
                flags.analog_classes.insert(simple.clone());
            }
        }
    }
    Ok(flags)
}

fn constant_bool(code: &[u8]) -> Option<bool> {
    let ins = decode(code).ok()?;
    let mut last_bool = None;
    for ins in &ins {
        match ins.op {
            0x03 => last_bool = Some(true),
            0x02 => last_bool = Some(false),
            0xac => return last_bool,
            _ => {}
        }
    }
    None
}

/// `sturdy` mask: `SupportType.FULL/CENTER/RIGID` x `Direction.values()`.
fn sturdy_mask(shape: &geometry::Shape, center: &geometry::Shape, rigid: &geometry::Shape) -> i64 {
    let mut mask = 0i64;
    for (support, comp) in [(0usize, None), (1, Some(center)), (2, Some(rigid))] {
        for direction in 0..6usize {
            let held = match comp {
                None => face_is_full(shape, direction),
                Some(test) => !only_second_nonempty(shape, direction, test),
            };
            if held {
                mask |= 1 << (support * 6 + direction);
            }
        }
    }
    mask
}

fn axis_of(direction: usize) -> (usize, bool) {
    match direction {
        0 => (1, false), // DOWN
        1 => (1, true),  // UP
        2 => (2, false), // NORTH
        3 => (2, true),  // SOUTH
        4 => (0, false), // WEST
        _ => (0, true),  // EAST
    }
}

/// A shape's boundary slice (`VoxelShape.getFaceShape`) membership test.
struct Slice<'a> {
    shape: &'a geometry::Shape,
    axis: usize,
    positive: bool,
}

impl Slice<'_> {
    fn contains(&self, point: [f64; 3]) -> bool {
        let (axis, positive) = (self.axis, self.positive);
        let sizes = [
            self.shape.x.len().saturating_sub(1),
            self.shape.y.len().saturating_sub(1),
            self.shape.z.len().saturating_sub(1),
        ];
        if sizes[axis] == 0 {
            return false;
        }
        let slice = if positive { sizes[axis] - 1 } else { 0 };
        let mut cell = [0usize; 3];
        for index in 0..3 {
            if index == axis {
                cell[index] = slice;
                continue;
            }
            let coords = axis_coords(self.shape, index);
            if point[index] < coords[0] || point[index] > coords[coords.len() - 1] {
                return false;
            }
            let after = coords.partition_point(|value| *value <= point[index]);
            if after == 0 || after > coords.len() - 1 {
                return false;
            }
            cell[index] = after - 1;
        }
        self.shape.occupied[(cell[0] * sizes[1] + cell[1]) * sizes[2] + cell[2]] != 0
    }
}

fn face_is_full(shape: &geometry::Shape, direction: usize) -> bool {
    let (axis, positive) = axis_of(direction);
    // `getFaceShape` presents the sliced axis as `[0, 1]`; `isShapeFullBlock`
    // then requires the other axes to be `[0, 1]` too, leaving a 1x1x1 grid.
    for index in 0..3 {
        let coords = axis_coords(shape, index);
        if coords.len() != 2 || coords[0] != 0.0 || coords[1] != 1.0 {
            return false;
        }
    }
    let sizes = [
        shape.x.len().saturating_sub(1),
        shape.y.len().saturating_sub(1),
        shape.z.len().saturating_sub(1),
    ];
    if sizes[axis] == 0 {
        return false;
    }
    let layer = if positive { sizes[axis] - 1 } else { 0 };
    let mut cell = [0usize; 3];
    cell[axis] = layer;
    shape.occupied[(cell[0] * sizes[1] + cell[1]) * sizes[2] + cell[2]] != 0
}
fn only_second_nonempty(shape: &geometry::Shape, direction: usize, other: &geometry::Shape) -> bool {
    let (axis, positive) = axis_of(direction);
    let slice = Slice { shape, axis, positive };
    let mut coords = [Vec::new(), Vec::new(), Vec::new()];
    for index in 0..3 {
        let mut values = axis_coords(shape, index).to_vec();
        values.extend(axis_coords(other, index).iter().copied());
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        values.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        coords[index] = values;
    }
    let sizes = [
        coords[0].len().saturating_sub(1),
        coords[1].len().saturating_sub(1),
        coords[2].len().saturating_sub(1),
    ];
    for x in 0..sizes[0] {
        for y in 0..sizes[1] {
            for z in 0..sizes[2] {
                let point = [
                    (coords[0][x] + coords[0][x + 1]) / 2.0,
                    (coords[1][y] + coords[1][y + 1]) / 2.0,
                    (coords[2][z] + coords[2][z + 1]) / 2.0,
                ];
                if contains(other, point) && !slice.contains(point) {
                    return true;
                }
            }
        }
    }
    false
}

fn axis_coords(shape: &geometry::Shape, axis: usize) -> &[f64] {
    match axis {
        0 => &shape.x,
        1 => &shape.y,
        _ => &shape.z,
    }
}

fn cell_center(shape: &geometry::Shape, cell: [usize; 3]) -> [f64; 3] {
    let coords = [
        axis_coords(shape, 0),
        axis_coords(shape, 1),
        axis_coords(shape, 2),
    ];
    [
        (coords[0][cell[0]] + coords[0][cell[0] + 1]) / 2.0,
        (coords[1][cell[1]] + coords[1][cell[1] + 1]) / 2.0,
        (coords[2][cell[2]] + coords[2][cell[2] + 1]) / 2.0,
    ]
}

fn contains(shape: &geometry::Shape, point: [f64; 3]) -> bool {
    let sizes = [
        shape.x.len().saturating_sub(1),
        shape.y.len().saturating_sub(1),
        shape.z.len().saturating_sub(1),
    ];
    let coords = [&shape.x, &shape.y, &shape.z];
    let mut cell = [0usize; 3];
    for index in 0..3 {
        if point[index] < coords[index][0] || point[index] > coords[index][coords[index].len() - 1] {
            return false;
        }
        let after = coords[index].partition_point(|value| *value <= point[index]);
        if after == 0 || after > coords[index].len() - 1 {
            return false;
        }
        cell[index] = after - 1;
    }
    shape.occupied[(cell[0] * sizes[1] + cell[1]) * sizes[2] + cell[2]] != 0
}

fn shape_from_boxes(boxes: &[(f64, f64, f64, f64, f64, f64)]) -> geometry::Shape {
    let mut shape = voxelshape::VoxelShape::empty();
    for (x1, y1, z1, x2, y2, z2) in boxes {
        shape = shape.or(voxelshape::VoxelShape::block_box(
            *x1, *y1, *z1, *x2, *y2, *z2,
        ));
    }
    let encoded = shape.encode();
    geometry::Shape {
        x: encoded.x,
        y: encoded.y,
        z: encoded.z,
        canonical: encoded.canonical,
        cube: encoded.cube,
        cube_axes: encoded.cube_axes,
        discrete_min: encoded.discrete_min,
        discrete_max: encoded.discrete_max,
        occupied: encoded.occupied,
    }
}

fn direction_ordinal(name: &str) -> Option<usize> {
    match name {
        "down" => Some(0),
        "up" => Some(1),
        "north" => Some(2),
        "south" => Some(3),
        "west" => Some(4),
        "east" => Some(5),
        _ => None,
    }
}

fn dir_mask(direction: Option<usize>, value: i32) -> i64 {
    direction
        .map(|direction| ((value as i64) & 15) << (4 * direction))
        .unwrap_or(0)
}

fn all_dirs(value: i32) -> i64 {
    (0..6).fold(0i64, |mask, direction| mask | dir_mask(Some(direction), value))
}

fn attached_direction(values: &BTreeMap<String, String>) -> Option<usize> {
    if let Some(face) = values.get("face").map(String::as_str) {
        match face {
            "floor" => return direction_ordinal("up"),
            "ceiling" => return direction_ordinal("down"),
            _ => {
                return values.get("facing").and_then(|facing| direction_ordinal(facing));
            }
        }
    }
    values.get("facing").and_then(|facing| direction_ordinal(facing))
}

fn signal_masks(chain: &[String], values: &BTreeMap<String, String>) -> (i64, i64) {
    let has = |class: &str| chain.iter().any(|entry| entry == class);
    let powered = property_true(values, "powered");
    let lit = property_true(values, "lit");
    let power = property_int(values, "power").unwrap_or(0).clamp(0, 15);
    if has("RedstoneWireBlock") {
        let weak = power as i64 * 0x111110;
        return (weak, weak);
    }
    if has("WeightedPressurePlateBlock") {
        return (power as i64 * 0x111111, dir_mask(Some(1), power));
    }
    if has("BasePressurePlateBlock") || has("PressurePlateBlock") || has("DetectorRailBlock") {
        if powered {
            return (all_dirs(15), dir_mask(Some(1), 15));
        }
        return (0, 0);
    }
    if has("LeverBlock") || has("ButtonBlock") {
        if powered {
            return (all_dirs(15), dir_mask(attached_direction(values), 15));
        }
        return (0, 0);
    }
    if has("RedstoneWallTorchBlock") {
        if lit {
            let facing = values.get("facing").and_then(|facing| direction_ordinal(facing));
            let weak = all_dirs(15) & !dir_mask(facing, 15);
            return (weak, dir_mask(Some(0), 15));
        }
        return (0, 0);
    }
    if has("RedstoneTorchBlock") {
        if lit {
            return (all_dirs(15) & !dir_mask(Some(1), 15), dir_mask(Some(0), 15));
        }
        return (0, 0);
    }
    if has("RepeaterBlock") || has("ObserverBlock") {
        if powered {
            let direction = values.get("facing").and_then(|facing| direction_ordinal(facing));
            let mask = dir_mask(direction, 15);
            return (mask, mask);
        }
        return (0, 0);
    }
    if has("TripWireHookBlock") {
        if powered {
            let direction = values.get("facing").and_then(|facing| direction_ordinal(facing));
            return (all_dirs(15), dir_mask(direction, 15));
        }
        return (0, 0);
    }
    if has("LightningRodBlock") || has("WeatheringLightningRodBlock") {
        if powered {
            let direction = values.get("facing").and_then(|facing| direction_ordinal(facing));
            return (all_dirs(15), dir_mask(direction, 15));
        }
        return (0, 0);
    }
    if has("DaylightDetectorBlock") || has("TargetBlock") {
        return (power as i64 * 0x111111, 0);
    }
    if has("SculkSensorBlock") {
        return (power as i64 * 0x111111, dir_mask(Some(1), power));
    }
    if has("CalibratedSculkSensorBlock") {
        let direction = values.get("direction").and_then(|direction| direction_ordinal(direction));
        let weak = (0..6).fold(0i64, |mask, dir| {
            if Some(dir) == direction {
                mask
            } else {
                mask | dir_mask(Some(dir), power)
            }
        });
        return (weak, dir_mask(Some(1), power));
    }
    if has("PoweredBlock") {
        return (all_dirs(15), 0);
    }
    if has("LecternBlock") {
        if property_true(values, "has_book") {
            return (all_dirs(15), dir_mask(Some(1), 15));
        }
        return (0, 0);
    }
    (0, 0)
}


/// `Properties.isSuffocating` overrides in `Blocks.<clinit>`; the default
/// predicate is tag-based, but the tag manager is not loaded by the exporter's
/// bootstrap, so only the explicit predicates answer.
fn suffocating_rule(chain: &[String], values: &BTreeMap<String, String>) -> bool {
    let has = |class: &str| chain.iter().any(|entry| entry == class);
    if has("ShulkerBoxBlock") {
        return true;
    }
    if has("PistonBaseBlock") {
        return !property_true(values, "extended");
    }
    has("FarmlandBlock") || has("SoulSandBlock") || has("PathBlock") || has("MudBlock")
}

/// Block ids in registry-state order (diagnostics).
pub fn state_block_ids(jar_path: &Path) -> Result<Vec<String>, String> {
    let registry = blockmod::registry(jar_path)?;
    let table = statemod::state_table(jar_path)?;
    let mut out = Vec::new();
    for (index, states) in table.blocks.iter().enumerate() {
        for _ in 0..states.state_count {
            out.push(registry[index].id.clone());
        }
    }
    Ok(out)
}
