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

use crate::classfile;
use crate::classfile::ClassFile;

#[path = "shape_final.rs"]
mod shape_final;

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

fn block_shape() -> geometry::Shape {
    geometry::Shape {
        x: vec![0.0, 1.0],
        y: vec![0.0, 1.0],
        z: vec![0.0, 1.0],
        canonical: 2,
        cube: 1,
        cube_axes: 7,
        discrete_min: [0, 0, 0],
        discrete_max: [1, 1, 1],
        occupied: vec![1],
    }
}

fn empty_shape() -> geometry::Shape {
    geometry::Shape::empty()
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
    Always,
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
            "PowderSnowBlock" => Pathfindable::Always,
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

#[derive(Clone, Copy, Debug, Default)]
struct Overrides {
    spawn: Option<SpawnKind>,
    redstone: Option<RedstoneKind>,
    light: Option<LightKind>,
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
    let overrides_by_method = method_overrides(&blocks);
    let mut next_index = 0usize;
    let mut notes = Vec::new();
    let clinit = blocks
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
        .map(|code| code.bytes.clone())
        .ok_or_else(|| "Blocks.<clinit> has no code".to_string())?;
    let ins = decode(&clinit)?;
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
                        let props = match value {
                            ScanVal::Props(props) => props,
                            _ => Overrides::default(),
                        };
                        if next_index < values.len() {
                            values[next_index] = props;
                            next_index += 1;
                        }
                        let _ = name;
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
                if (owner.ends_with("ColorCollection")
                    || owner.ends_with("WeatheringCopperCollection"))
                    && name == "registerBlocks"
                {
                    let count = if owner.ends_with("ColorCollection") { 16 } else { 8 };
                    let props = arguments
                        .iter()
                        .rev()
                        .find_map(|value| match value {
                            ScanVal::Func(function) => {
                                overrides_by_method.get(function).copied()
                            }
                            _ => None,
                        })
                        .unwrap_or_default();
                    for offset in 0..count {
                        if next_index + offset < values.len() {
                            values[next_index + offset] = props;
                        }
                    }
                    next_index += count;
                    continue;
                }
                if owner.ends_with("Blocks") && returns_block(&descriptor) {
                    let mut props = arguments
                        .iter()
                        .find_map(|value| match value {
                            ScanVal::Props(props) => Some(*props),
                            _ => None,
                        })
                        .unwrap_or_default();
                    if let Some(helper) = overrides_by_method.get(&name).copied() {
                        props.spawn = props.spawn.or(helper.spawn);
                        props.redstone = props.redstone.or(helper.redstone);
                        props.light = props.light.or(helper.light);
                    }
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
                    stack.push(ScanVal::Props(props));
                    continue;
                }
                if returns_properties(&descriptor) {
                    let props = overrides_by_method
                        .get(&name)
                        .copied()
                        .unwrap_or_default();
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
                    let source_overrides = values[source];
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

fn method_overrides(blocks: &ClassFile) -> BTreeMap<String, Overrides> {
    let mut map: BTreeMap<String, Overrides> = BTreeMap::new();
    for method in &blocks.methods {
        let Some(code) = method.code.as_ref() else { continue };
        let Ok(ins) = decode(&code.bytes) else { continue };
        let mut props = Overrides::default();
        let mut last_func: Option<String> = None;
        let mut last_light: Option<LightKind> = None;
        for (index, current) in ins.iter().enumerate() {
            match current.op {
                0xba => {
                    last_func = indy_target(blocks, current.operand as usize);
                }
                0xb8 => {
                    if let Some((owner, name, _descriptor)) =
                        classfile::method_ref(&blocks.constants, current.operand as usize)
                    {
                        if owner.ends_with("Blocks") {
                            if name == "litBlockEmission" {
                                let value = index
                                    .checked_sub(1)
                                    .and_then(|previous| ins.get(previous))
                                    .map(|instruction| match instruction.op {
                                        0x02..=0x08 => instruction.op as i32 - 3,
                                        0x10 | 0x11 => instruction.operand,
                                        _ => 15,
                                    })
                                    .unwrap_or(15);
                                last_light = Some(LightKind::Lit(value));
                            } else if name == "always"
                                || name == "never"
                                || name == "ocelotOrParrot"
                                || name.starts_with("lambda$static$")
                            {
                                last_func = Some(name.clone());
                            }
                        }
                    }
                }
                0xb6 => {
                    if let Some((owner, name, _descriptor)) =
                        classfile::method_ref(&blocks.constants, current.operand as usize)
                    {
                        if owner.ends_with("Properties") {
                            match name.as_str() {
                                "isValidSpawn" => {
                                    if let Some(function) = last_func.take() {
                                        props.spawn = Some(classify_spawn(&function));
                                    }
                                }
                                "isRedstoneConductor" => {
                                    if let Some(function) = last_func.take() {
                                        props.redstone = Some(classify_redstone(&function));
                                    }
                                }
                                "lightLevel" => {
                                    let kind = last_light.take().or_else(|| {
                                        last_func
                                            .take()
                                            .map(|function| classify_light(blocks, &function))
                                    });
                                    props.light = Some(kind.unwrap_or(LightKind::Unknown));
                                }
                                _ => {}
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        map.insert(method.name.clone(), props);
    }
    for _ in 0..6 {
        for method in &blocks.methods {
            let Some(code) = method.code.as_ref() else { continue };
            let Ok(ins) = decode(&code.bytes) else { continue };
            let mut merged = map.get(&method.name).copied().unwrap_or_default();
            for current in &ins {
                if current.op == 0xb8 {
                    if let Some((owner, name, _descriptor)) =
                        classfile::method_ref(&blocks.constants, current.operand as usize)
                    {
                        if owner.ends_with("Blocks") {
                            if let Some(callee) = map.get(&name).copied() {
                                merged.spawn = merged.spawn.or(callee.spawn);
                                merged.redstone = merged.redstone.or(callee.redstone);
                                merged.light = merged.light.or(callee.light);
                            }
                        }
                    }
                }
            }
            map.insert(method.name.clone(), merged);
        }
    }
    map
}

fn returns_properties(descriptor: &str) -> bool {
    classfile::descriptor_types(descriptor)
        .last()
        .map(|kind| kind.contains("BlockBehaviour$Properties"))
        .unwrap_or(false)
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

/// Debug helper: `shape`'s block property table for one block.
pub fn debug_props(jar_path: &Path, id: &str) -> Result<String, String> {
    let registry = blockmod::registry(jar_path)?;
    let properties = shape_props::tables(jar_path)?;
    for (index, block) in registry.iter().enumerate() {
        if block.id == format!("minecraft:{id}") {
            return Ok(format!("{} -> {:?}", block.id, properties.blocks[index].1));
        }
    }
    Err("not found".to_string())
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
            .copied()
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
        let can_occlude = properties
            .blocks
            .get(block_index)
            .map(|(_, props)| props.can_occlude)
            .unwrap_or(true);
        let collision_enabled = has_collision.get(block_index).copied().unwrap_or(true);
        let behaviour = pathfindable_behavior(&block.chain);
        let signal = is_signal_source(&block.chain);
        let can_open_by_hand = block.id != "minecraft:iron_door";
        for offset in 0..states.state_count {
            let state_index = rows.len();
            let values = state_values(states, offset);
            let fluid = fluid_state(&block.chain, &block.id, &values);
            let mut collision = evaluated.collision.get(state_index).cloned().flatten();
            let mut occlusion = evaluated.occlusion.get(state_index).cloned().flatten();
            let mut support = evaluated.support.get(state_index).cloned().flatten();
            if collision.is_none() {
                // Dynamic shapes are uncached in the prepared tables; the
                // exporter still calls getCollisionShape at ZERO with an empty
                // level. The shulker box override answers Shapes.block(), the
                // moving piston and powder snow answer Shapes.empty().
                if block.id.contains("shulker_box") {
                    collision = Some(block_shape());
                    support = Some(block_shape());
                } else if block.id == "minecraft:moving_piston" || block.id == "minecraft:powder_snow" {
                    collision = Some(empty_shape());
                    support = Some(empty_shape());
                }
            }
            let _ = &mut occlusion;
            let collision_full = collision
                .as_ref()
                .map(shape_is_full)
                .unwrap_or(false);
            let solid_render = can_occlude && occlusion.as_ref().map(shape_is_full).unwrap_or(false);
            let face_sturdy_up = support.as_ref().map(shape_face_full_up).unwrap_or(false);
            let light = light_emission(overrides.light, &values);
            let mut flags = 0i32;
            for (bit, kind) in [
                (1, PathKind::Land),
                (2, PathKind::Water),
                (4, PathKind::Air),
            ] {
                if pathfindable(
                    behaviour,
                    &values,
                    fluid,
                    collision_full,
                    collision_enabled,
                    kind,
                ) {
                    flags |= bit;
                }
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
                RedstoneKind::Default => collision_full,
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
                    let size = ((bounds[3] - bounds[0])
                        + (bounds[4] - bounds[1])
                        + (bounds[5] - bounds[2]))
                        / 3.0;
                    size >= 0.7291666666666666 || (bounds[4] - bounds[1]) >= 1.0
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum PathKind {
    Land,
    Water,
    Air,
}

fn pathfindable(
    behaviour: Pathfindable,
    values: &BTreeMap<String, String>,
    fluid: Fluid,
    collision_full: bool,
    collision_enabled: bool,
    kind: PathKind,
) -> bool {
    match behaviour {
        Pathfindable::Default => match kind {
            PathKind::Land | PathKind::Air => !collision_full,
            PathKind::Water => fluid == Fluid::Water,
        },
        Pathfindable::Never => false,
        Pathfindable::Always => true,
        Pathfindable::Door => match kind {
            PathKind::Water => false,
            _ => property_true(values, "open"),
        },
        Pathfindable::TrapDoor => match kind {
            PathKind::Land | PathKind::Air => property_true(values, "open"),
            PathKind::Water => property_true(values, "waterlogged"),
        },
        Pathfindable::FenceGate => match kind {
            PathKind::Water => false,
            _ => property_true(values, "open"),
        },
        Pathfindable::Slab => match kind {
            PathKind::Land | PathKind::Air => false,
            PathKind::Water => fluid == Fluid::Water,
        },
        Pathfindable::SnowLayer => match kind {
            PathKind::Land => property_int(values, "layers").unwrap_or(1) < 5,
            _ => false,
        },
        Pathfindable::Liquid => fluid != Fluid::Lava,
        Pathfindable::WaterOnly => match kind {
            PathKind::Water => fluid == Fluid::Water,
            _ => false,
        },
        Pathfindable::Vegetation => match kind {
            PathKind::Land => !collision_full,
            PathKind::Water => fluid == Fluid::Water,
            PathKind::Air => !collision_enabled || !collision_full,
        },
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
    if !pathfindable(
        behaviour,
        values,
        fluid,
        collision_full,
        true,
        PathKind::Land,
    ) {
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
