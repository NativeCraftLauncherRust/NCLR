//! `mob-data.json#states` predicate flags, rebuilt from the player's own
//! `client.jar`.
//!
//! `VanillaMobDataExporter.states()` builds the integer in column 1 from
//! vanilla's state accessors:
//!
//! ```text
//! bit 3  (8)    state.isValidSpawn(EMPTY, ZERO, EntityTypes.ZOMBIE)
//! bit 4  (16)   state.isValidSpawn(EMPTY, ZERO, EntityTypes.OCELOT)
//! bit 5  (32)   state.isValidSpawn(EMPTY, ZERO, EntityTypes.POLAR_BEAR)
//! bit 6  (64)   state.isRedstoneConductor(EMPTY, ZERO)
//! bit 9  (512)  state.isSolid()
//! bit 10 (1024) state.isSolidRender()
//! ```
//!
//! `EMPTY` is `EmptyBlockGetter.INSTANCE` at `BlockPos.ZERO`: neighbours are air
//! and block entities are `null`, which is why e.g. a comparator has no output
//! signal and a redstone-wire dot only powers upwards.
//!
//! # Derived facts
//!
//! * `isValidSpawn` defaults to
//!   `state.isFaceSturdy(level, pos, UP) && state.getLightEmission() < 14`
//!   (`BlockBehaviour$Properties.lambda$new$4`); `isRedstoneConductor` defaults
//!   to `state.isCollisionShapeFullBlock(level, pos)`
//!   (`BlockBehaviour$Properties` bootstrap #5).
//! * Both, plus `lightLevel`, `forceSolidOn/Off`, `noCollision`, `noOcclusion`
//!   and `dynamicShape`, are `Properties` fields built in `Blocks.<clinit>`.
//!   This module *interprets* that initialiser: `Properties.of()...` chains,
//!   the helper methods (`leavesProperties`, `wallVariant`, `registerStair`,
//!   ...), `ofLegacyCopy`/`ofFullCopy` (which copy different field sets), and
//!   the colour/copper collection registrations (whose property functions run
//!   once per `DyeColor`/`WeatherState`) are all evaluated, including the
//!   invokedynamic closures.
//! * `isSolid` is `BlockStateBase.calculateSolid`: `forceSolidOn`, then
//!   `forceSolidOff`, then a dynamic shape (null cache) fails, then an empty
//!   collision shape fails, then `AABB.getSize() >= 35/48` or
//!   `AABB.getYsize() >= 1`.
//! * `isSolidRender` is `BlockStateBase.solidRender`: `canOcclude` and the
//!   occlusion shape is a full block.
//!
//! Shapes come from `shape_final::evaluate` (the crate's shape-engine
//! integration); this module only evaluates the predicate/`Properties` side.
//!
//! `tests/mob_flags.rs` compares the six bits against
//! `tmp/ship-extract/mob-data.json` for every registry state and reports the
//! exact `flags_bad` count.

#![allow(dead_code, clippy::duplicate_mod)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use crate::classfile;
use classfile::{ClassFile, Constant};

#[path = "shape_final.rs"]
mod shape_final;

use shape_final::block as blockmod;
use shape_final::geometry;
use shape_final::states as statemod;

// ---------------------------------------------------------------------------
// API
// ---------------------------------------------------------------------------

pub const SPAWN_ZOMBIE: i32 = 1 << 3;
pub const SPAWN_OCELOT: i32 = 1 << 4;
pub const SPAWN_POLAR_BEAR: i32 = 1 << 5;
pub const REDSTONE_CONDUCTOR: i32 = 1 << 6;
pub const SOLID: i32 = 1 << 9;
pub const SOLID_RENDER: i32 = 1 << 10;

/// The bits this module produces.
pub const PREDICATE_FLAGS: i32 =
    SPAWN_ZOMBIE | SPAWN_OCELOT | SPAWN_POLAR_BEAR | REDSTONE_CONDUCTOR | SOLID | SOLID_RENDER;

/// What the derivation produced and what it could not.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub states: usize,
    pub blocks: usize,
    /// Blocks with an unresolved predicate/light function.
    pub gaps: Vec<String>,
    pub notes: Vec<String>,
}

/// The six predicate bits of every registry state, in registry order.
pub fn flags(jar_path: &Path) -> Result<(Vec<i32>, Coverage), String> {
    let registry = blockmod::registry(jar_path)?;
    let table = statemod::state_table(jar_path)?;
    if registry.len() != table.blocks.len() {
        return Err("block registry and state table disagree".to_string());
    }
    let evaluated = shape_final::evaluate(jar_path)?;
    if evaluated.collision.len() != table.state_count() {
        return Err("evaluated collision shapes and states disagree".to_string());
    }
    let scan = scan_blocks(jar_path, &registry)?;
    if scan.registrations.len() != registry.len() {
        return Err(format!(
            "Blocks.<clinit> registered {} blocks, the registry has {}",
            scan.registrations.len(),
            registry.len()
        ));
    }
    let debug = std::env::var_os("MOB_FLAGS_DEBUG_FILE");
    let mut coverage = Coverage {
        states: table.state_count(),
        blocks: registry.len(),
        gaps: scan.gaps.clone(),
        notes: scan.notes,
    };
    let mut debug_lines: Vec<String> = Vec::new();
    let mut rows = Vec::with_capacity(table.state_count());
    let mut state_index = 0usize;
    for (block_index, states) in table.blocks.iter().enumerate() {
        let raw = scan
            .registrations
            .get(block_index)
            .cloned()
            .ok_or_else(|| format!("registration {block_index} is missing"))?;
        let props = resolve_props(&raw, &scan.fields, 0);
        let block = &registry[block_index];
        let dynamic = props.dynamic_shape == Some(true)
            || block.chain.iter().any(|class| scan.dynamic.contains(class));
        let unknown_redstone = matches!(props.redstone, Some(RedstonePred::Unknown));
        let unknown_spawn = matches!(props.spawn, Some(SpawnPred::Unknown));
        if unknown_redstone {
            coverage.gaps.push(format!(
                "{}: unknown isRedstoneConductor",
                registry[block_index].id
            ));
        }
        if unknown_spawn {
            coverage.gaps.push(format!(
                "{}: unknown isValidSpawn",
                registry[block_index].id
            ));
        }
        for offset in 0..states.state_count {
            let values = state_values(states, offset);
            let mut collision = evaluated.collision.get(state_index).cloned().flatten();
            let occlusion = evaluated.occlusion.get(state_index).cloned().flatten();
            let mut support = evaluated.support.get(state_index).cloned().flatten();
            if collision.is_none() {
                // Dynamic shapes have no cached shape; the exporter still asks
                // `getCollisionShape` at ZERO: the shulker box answers a full
                // block, the moving piston and powder snow answer empty.
                if block.id.contains("shulker_box") {
                    collision = Some(block_shape());
                    support = Some(block_shape());
                } else if block.id == "minecraft:moving_piston"
                    || block.id == "minecraft:powder_snow"
                {
                    collision = Some(empty_shape());
                    support = Some(empty_shape());
                }
            }
            let has_collision = props.has_collision.unwrap_or(true);
            let collision = if has_collision { collision } else { None };
            let collision_empty = collision.as_ref().map(shape_is_empty).unwrap_or(true);
            let collision_full = collision.as_ref().map(shape_is_full).unwrap_or(false);
            let light = light_of(&props.light, &values);
            let face_full_up = support.as_ref().map(shape_face_full_up).unwrap_or(false);
            let mut flags = 0i32;
            for (bit, entity) in [
                (SPAWN_ZOMBIE, Entity::Zombie),
                (SPAWN_OCELOT, Entity::Ocelot),
                (SPAWN_POLAR_BEAR, Entity::PolarBear),
            ] {
                let valid = match props.spawn {
                    Some(predicate) => predicate.spawns(entity, face_full_up, light),
                    None => face_full_up && light < 14,
                };
                if valid {
                    flags |= bit;
                }
            }
            let redstone = match props.redstone {
                Some(RedstonePred::Never) => false,
                Some(RedstonePred::Always) => true,
                Some(RedstonePred::Default) | Some(RedstonePred::Unknown) | None => collision_full,
            };
            if redstone {
                flags |= REDSTONE_CONDUCTOR;
            }
            let solid = if props.force_solid_on == Some(true) {
                true
            } else if props.force_solid_off == Some(true) || dynamic || collision_empty {
                false
            } else if let Some(collision) = collision.as_ref() {
                let bounds = shape_bounds(collision);
                let size =
                    ((bounds[3] - bounds[0]) + (bounds[4] - bounds[1]) + (bounds[5] - bounds[2]))
                        / 3.0;
                size >= 0.7291666666666666 || (bounds[4] - bounds[1]) >= 1.0
            } else {
                false
            };
            if solid {
                flags |= SOLID;
            }
            let can_occlude = props.can_occlude.unwrap_or(true);
            if can_occlude && occlusion.as_ref().map(shape_is_full).unwrap_or(false) {
                flags |= SOLID_RENDER;
            }
            if debug.is_some() && offset == 0 {
                debug_lines.push(format!(
                    "{} spawn={:?} red={:?} light={:?} on={:?} off={:?} occl={:?} coll={:?} dyn={dynamic} c={} {:?} o={} {:?} s={} {:?} flags={:#x}",
                    block.id,
                    props.spawn,
                    props.redstone,
                    props.light,
                    props.force_solid_on,
                    props.force_solid_off,
                    props.can_occlude,
                    props.has_collision,
                    shape_kind(collision.as_ref()),
                    collision.as_ref().map(shape_bounds),
                    shape_kind(occlusion.as_ref()),
                    occlusion.as_ref().map(shape_bounds),
                    shape_kind(support.as_ref()),
                    support.as_ref().map(shape_bounds),
                    flags,
                ));
            }
            rows.push(flags & PREDICATE_FLAGS);
            state_index += 1;
        }
    }
    coverage.gaps.sort();
    coverage.gaps.dedup();
    if let Some(path) = debug {
        let _ = fs::write(path, debug_lines.join("\n"));
    }
    Ok((rows, coverage))
}

/// Block id for every registry state, in the same order as [`flags`].
pub fn state_block_ids(jar_path: &Path) -> Result<Vec<String>, String> {
    let registry = blockmod::registry(jar_path)?;
    let table = statemod::state_table(jar_path)?;
    let mut out = Vec::with_capacity(table.state_count());
    for (block, states) in registry.iter().zip(table.blocks.iter()) {
        for _ in 0..states.state_count {
            out.push(block.id.clone());
        }
    }
    Ok(out)
}

/// Diagnostic: how many blocks `Blocks.<clinit>` registers versus the registry.
pub fn scan_counts(jar_path: &Path) -> Result<(usize, usize, Vec<String>, Vec<String>), String> {
    let registry = blockmod::registry(jar_path)?;
    let scan = scan_blocks(jar_path, &registry)?;
    Ok((
        scan.registrations.len(),
        registry.len(),
        scan.gaps,
        scan.notes,
    ))
}

// ---------------------------------------------------------------------------
// Predicates decoded from the jar's closures
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entity {
    Zombie,
    Ocelot,
    PolarBear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnPred {
    /// `isFaceSturdy(UP) && getLightEmission() < 14`.
    Default,
    Never,
    Always,
    OcelotOrParrot,
    PolarBear,
    ZombieOnly,
    FireImmune,
    Unknown,
}

impl SpawnPred {
    fn spawns(self, entity: Entity, face_full_up: bool, light: i32) -> bool {
        match self {
            SpawnPred::Default => face_full_up && light < 14,
            SpawnPred::Never => false,
            SpawnPred::Always => true,
            SpawnPred::OcelotOrParrot => entity == Entity::Ocelot,
            SpawnPred::PolarBear => entity == Entity::PolarBear,
            SpawnPred::ZombieOnly => entity == Entity::Zombie,
            // None of the three overworld probes is fire immune.
            SpawnPred::FireImmune | SpawnPred::Unknown => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedstonePred {
    Default,
    Never,
    Always,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightFun {
    Constant(i32),
    Lit(i32),
    RespawnAnchor,
    SeaPickle,
    Zero,
    Property(&'static str),
}

fn light_of(kind: &Option<LightFun>, values: &BTreeMap<String, String>) -> i32 {
    match *kind {
        None | Some(LightFun::Zero) => 0,
        Some(LightFun::Constant(value)) => value,
        Some(LightFun::Lit(value)) => {
            if value_of(values, "lit") == Some("true") {
                value
            } else {
                0
            }
        }
        Some(LightFun::RespawnAnchor) => {
            let charges = int_of(values, "charges").clamp(0, 4);
            charges * 15 / 4
        }
        Some(LightFun::SeaPickle) => {
            let pickles = int_of(values, "pickles").max(1);
            3 + 3 * pickles
        }
        Some(LightFun::Property(name)) => int_of(values, name),
    }
}

// ---------------------------------------------------------------------------
// Properties chain model
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
struct Props {
    spawn: Option<SpawnPred>,
    redstone: Option<RedstonePred>,
    light: Option<LightFun>,
    force_solid_on: Option<bool>,
    force_solid_off: Option<bool>,
    can_occlude: Option<bool>,
    has_collision: Option<bool>,
    dynamic_shape: Option<bool>,
    copy: Option<(CopySource, CopyKind)>,
}

#[derive(Clone, Debug)]
enum CopySource {
    /// `Blocks.<FIELD>`, resolved once the scan has seen the base registration.
    Field(String),
    /// A block value already in hand (`Collection.pick(...)`).
    Inline(Box<Props>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CopyKind {
    /// `Properties.ofLegacyCopy`: instance fields, but the spawn/redstone
    /// predicates go back to their defaults.
    Legacy,
    /// `Properties.ofFullCopy`: legacy copy plus the two predicates.
    Full,
}

fn inherit(raw: &Props, base: &Props, kind: CopyKind) -> Props {
    let mut merged = base.clone();
    if raw.spawn.is_some() {
        merged.spawn = raw.spawn;
    }
    if raw.redstone.is_some() {
        merged.redstone = raw.redstone;
    }
    if raw.light.is_some() {
        merged.light = raw.light;
    }
    if raw.force_solid_on.is_some() {
        merged.force_solid_on = raw.force_solid_on;
    }
    if raw.force_solid_off.is_some() {
        merged.force_solid_off = raw.force_solid_off;
    }
    if raw.can_occlude.is_some() {
        merged.can_occlude = raw.can_occlude;
    }
    if raw.has_collision.is_some() {
        merged.has_collision = raw.has_collision;
    }
    if raw.dynamic_shape.is_some() {
        merged.dynamic_shape = raw.dynamic_shape;
    }
    if kind == CopyKind::Legacy {
        // `ofLegacyCopy` does not copy the two predicates.
        if raw.spawn.is_none() {
            merged.spawn = None;
        }
        if raw.redstone.is_none() {
            merged.redstone = None;
        }
    }
    merged.copy = None;
    merged
}

fn resolve_props(raw: &Props, fields: &BTreeMap<String, Props>, depth: usize) -> Props {
    if depth > 16 {
        return raw.clone();
    }
    let Some((source, kind)) = &raw.copy else {
        return raw.clone();
    };
    let base = match source {
        CopySource::Inline(props) => resolve_props(props, fields, depth + 1),
        CopySource::Field(name) => match fields.get(name) {
            Some(base) => resolve_props(base, fields, depth + 1),
            None => return raw.clone(),
        },
    };
    inherit(raw, &base, *kind)
}

struct Scan {
    registrations: Vec<Props>,
    fields: BTreeMap<String, Props>,
    collections: BTreeMap<String, Vec<Props>>,
    /// Classes overriding `Block.hasDynamicShape()` to `true`.
    dynamic: BTreeSet<String>,
    gaps: Vec<String>,
    notes: Vec<String>,
}

// ---------------------------------------------------------------------------
// Bytecode
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Ins {
    offset: usize,
    op: u8,
    a: i32,
    b: i32,
    /// tableswitch/lookupswitch payload: (match key, relative branch).
    cases: Vec<(i32, i32)>,
}

fn decode(bytes: &[u8]) -> Result<Vec<Ins>, String> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let offset = at;
        let op = bytes[at];
        let mut a = 0i32;
        let mut b = 0i32;
        let mut cases: Vec<(i32, i32)> = Vec::new();
        let length: usize = match op {
            0x00..=0x0f => 1,
            0x10 => {
                a = bytes.get(at + 1).copied().unwrap_or(0) as i8 as i32;
                2
            }
            0x11 => {
                a = read_s16(bytes, at + 1);
                3
            }
            0x12 => {
                a = bytes.get(at + 1).copied().unwrap_or(0) as i32;
                2
            }
            0x13 | 0x14 => {
                a = read_u16(bytes, at + 1);
                3
            }
            0x15..=0x19 | 0x36..=0x3a => {
                a = bytes.get(at + 1).copied().unwrap_or(0) as i32;
                2
            }
            0x1a..=0x35 | 0x3b..=0x83 => 1,
            0x84 => {
                a = bytes.get(at + 1).copied().unwrap_or(0) as i32;
                b = bytes.get(at + 2).copied().unwrap_or(0) as i8 as i32;
                3
            }
            0x85..=0x98 => 1,
            0x99..=0xa8 => {
                a = read_s16(bytes, at + 1);
                3
            }
            0xa9 => 2,
            0xaa | 0xab => {
                let mut pad = at + 1;
                while !pad.is_multiple_of(4) {
                    pad += 1;
                }
                a = read_i32(bytes, pad);
                if op == 0xaa {
                    let low = read_i32(bytes, pad + 4);
                    let high = read_i32(bytes, pad + 8);
                    let count = (high - low + 1).max(0) as usize;
                    for index in 0..count {
                        let key = low + index as i32;
                        let delta = read_i32(bytes, pad + 12 + 4 * index);
                        cases.push((key, delta));
                    }
                    pad - at + 12 + 4 * count
                } else {
                    let count = read_i32(bytes, pad + 4).max(0) as usize;
                    for index in 0..count {
                        let key = read_i32(bytes, pad + 8 + 8 * index);
                        let delta = read_i32(bytes, pad + 12 + 8 * index);
                        cases.push((key, delta));
                    }
                    pad - at + 8 + 8 * count
                }
            }
            0xac..=0xb1 => 1,
            0xb2..=0xb8 => {
                a = read_u16(bytes, at + 1);
                3
            }
            0xb9 | 0xba => {
                a = read_u16(bytes, at + 1);
                b = bytes.get(at + 3).copied().unwrap_or(0) as i32;
                5
            }
            0xbb => {
                a = read_u16(bytes, at + 1);
                3
            }
            0xbc => {
                a = bytes.get(at + 1).copied().unwrap_or(0) as i32;
                2
            }
            0xbd => {
                a = read_u16(bytes, at + 1);
                3
            }
            0xbe | 0xbf => 1,
            0xc0 | 0xc1 => {
                a = read_u16(bytes, at + 1);
                3
            }
            0xc2 | 0xc3 => 1,
            0xc4 => {
                let op2 = bytes.get(at + 1).copied().unwrap_or(0);
                a = bytes.get(at + 2).copied().unwrap_or(0) as i32;
                if op2 == 0x84 {
                    6
                } else {
                    4
                }
            }
            0xc5 => {
                a = read_u16(bytes, at + 1);
                4
            }
            0xc6 | 0xc7 => {
                a = read_s16(bytes, at + 1);
                3
            }
            0xc8 | 0xc9 => {
                a = read_i32(bytes, at + 1);
                5
            }
            _ => 1,
        };
        out.push(Ins {
            offset,
            op,
            a,
            b,
            cases,
        });
        at += length.max(1);
    }
    Ok(out)
}

fn read_u16(bytes: &[u8], at: usize) -> i32 {
    let hi = bytes.get(at).copied().unwrap_or(0) as i32;
    let lo = bytes.get(at + 1).copied().unwrap_or(0) as i32;
    (hi << 8) | lo
}

fn read_s16(bytes: &[u8], at: usize) -> i32 {
    read_u16(bytes, at) as i16 as i32
}

fn read_i32(bytes: &[u8], at: usize) -> i32 {
    let mut value = 0i32;
    for index in 0..4 {
        value = (value << 8) | bytes.get(at + index).copied().unwrap_or(0) as i32;
    }
    value
}

fn branch_at(ins: &[Ins], at: usize, delta: i32) -> Result<usize, String> {
    let target = ins[at].offset as i64 + delta as i64;
    ins.iter()
        .position(|instruction| instruction.offset as i64 == target)
        .ok_or_else(|| format!("branch target {target} is not an instruction"))
}

// ---------------------------------------------------------------------------
// The Blocks.<clinit> interpreter
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Val {
    Int(i32),
    Float(f32),
    Str(String),
    Props(Props),
    Light(LightFun),
    Block {
        field: Option<String>,
        props: Option<Props>,
    },
    Collection(Vec<Props>),
    /// A `$SwitchMap` table: enum constant name -> case label.
    SwitchMap(BTreeMap<String, i32>),
    /// `Enum.ordinal()` result, kept as the constant name.
    Ordinal(String),
    Func(Func),
    Object(String),
    EntityType(String),
    Tag(String),
    Property(String),
    EnumValue {
        owner: String,
        name: String,
    },
    Unknown,
}

#[derive(Clone, Debug)]
struct Func {
    owner: String,
    name: String,
    descriptor: String,
    captured: Vec<Val>,
}

impl Func {
    fn blocks(name: &str, descriptor: &str) -> Func {
        Func {
            owner: "net/minecraft/world/level/block/Blocks".to_string(),
            name: name.to_string(),
            descriptor: descriptor.to_string(),
            captured: Vec::new(),
        }
    }
}

struct Interp<'a> {
    jar: &'a mut Jar,
    registry: &'a [blockmod::Block],
    scan: Scan,
    depth: usize,
}

impl<'a> Interp<'a> {
    fn new(jar: &'a mut Jar, registry: &'a [blockmod::Block]) -> Interp<'a> {
        Interp {
            jar,
            registry,
            scan: Scan {
                registrations: Vec::new(),
                fields: BTreeMap::new(),
                collections: BTreeMap::new(),
                dynamic: BTreeSet::new(),
                gaps: Vec::new(),
                notes: Vec::new(),
            },
            depth: 0,
        }
    }

    fn run(mut self) -> Result<Scan, String> {
        let code = {
            let class = self.jar.class("net/minecraft/world/level/block/Blocks")?;
            class
                .method("<clinit>")
                .and_then(|method| method.code.as_ref())
                .map(|code| code.bytes.clone())
                .ok_or_else(|| "Blocks.<clinit> has no code".to_string())?
        };
        let ins = decode(&code)?;
        let mut stack: Vec<Val> = Vec::new();
        let mut locals: Vec<Val> = Vec::new();
        self.exec(&ins, &mut stack, &mut locals)?;
        Ok(self.scan)
    }

    fn exec(
        &mut self,
        ins: &[Ins],
        stack: &mut Vec<Val>,
        locals: &mut Vec<Val>,
    ) -> Result<Option<Val>, String> {
        let mut at = 0usize;
        while at < ins.len() {
            let current = ins[at].clone();
            let op = current.op;
            let mut next = at + 1;
            match op {
                0x00 => {}
                0x01 => stack.push(Val::Unknown),
                0x02..=0x08 => stack.push(Val::Int(op as i32 - 3)),
                0x09 | 0x0a | 0x0e | 0x0f => stack.push(Val::Unknown),
                0x0b..=0x0d => stack.push(Val::Float(op as f32 - 11.0)),
                0x10 | 0x11 => stack.push(Val::Int(current.a)),
                0x12 | 0x13 => {
                    let value = self.ldc(current.a)?;
                    stack.push(value);
                }
                0x14 => stack.push(Val::Unknown),
                0x15..=0x19 => {
                    let index = current.a as usize;
                    let value = locals.get(index).cloned().unwrap_or(Val::Unknown);
                    stack.push(value);
                }
                0x1a..=0x1d => {
                    let index = (op - 0x1a) as usize;
                    let value = locals.get(index).cloned().unwrap_or(Val::Unknown);
                    stack.push(value);
                }
                0x1e..=0x21 | 0x26..=0x29 => stack.push(Val::Unknown),
                0x22..=0x25 => {
                    let index = (op - 0x22) as usize;
                    let value = locals.get(index).cloned().unwrap_or(Val::Unknown);
                    stack.push(value);
                }
                0x2a..=0x2d => {
                    let index = (op - 0x2a) as usize;
                    let value = locals.get(index).cloned().unwrap_or(Val::Unknown);
                    stack.push(value);
                }
                0x36..=0x3a => {
                    let index = current.a as usize;
                    let value = stack.pop().unwrap_or(Val::Unknown);
                    set_local(locals, index, value);
                }
                0x3b..=0x4e => {
                    let index = (op - 0x3b) as usize % 4;
                    let value = stack.pop().unwrap_or(Val::Unknown);
                    set_local(locals, index, value);
                }
                0x57 => {
                    stack.pop();
                }
                0x58 => {
                    stack.pop();
                    stack.pop();
                }
                0x59 => {
                    let value = stack.last().cloned().unwrap_or(Val::Unknown);
                    stack.push(value);
                }
                0x5a..=0x5f => {
                    if stack.len() >= 2 {
                        let top = stack.last().cloned().unwrap_or(Val::Unknown);
                        let position = stack.len() - 2;
                        stack.insert(position, top);
                    }
                }
                0x60..=0x83 => {
                    let right = stack.pop().unwrap_or(Val::Unknown);
                    let left = stack.pop().unwrap_or(Val::Unknown);
                    stack.push(arithmetic(op, left, right));
                }
                0x84 => {
                    let index = current.a as usize;
                    if let Some(Val::Int(value)) = locals.get(index) {
                        let value = *value + current.b;
                        set_local(locals, index, Val::Int(value));
                    }
                }
                0x85..=0x93 => {
                    let value = stack.pop().unwrap_or(Val::Unknown);
                    stack.push(convert(op, value));
                }
                0x94..=0x98 => {
                    stack.pop();
                    stack.pop();
                    stack.push(Val::Int(0));
                }
                0x99..=0x9e => {
                    let value = stack.pop().unwrap_or(Val::Unknown);
                    if let Val::Int(value) = value {
                        let taken = match op {
                            0x99 => value == 0,
                            0x9a => value != 0,
                            0x9b => value < 0,
                            0x9c => value >= 0,
                            0x9d => value > 0,
                            _ => value <= 0,
                        };
                        if taken {
                            next = branch_at(ins, at, current.a)?;
                        }
                    }
                }
                0x9f..=0xa4 => {
                    let right = stack.pop().unwrap_or(Val::Unknown);
                    let left = stack.pop().unwrap_or(Val::Unknown);
                    if let (Val::Int(left), Val::Int(right)) = (left, right) {
                        let taken = match op {
                            0x9f => left == right,
                            0xa0 => left != right,
                            0xa1 => left < right,
                            0xa2 => left >= right,
                            0xa3 => left > right,
                            _ => left <= right,
                        };
                        if taken {
                            next = branch_at(ins, at, current.a)?;
                        }
                    }
                }
                0xa5 | 0xa6 => {
                    let right = stack.pop().unwrap_or(Val::Unknown);
                    let left = stack.pop().unwrap_or(Val::Unknown);
                    let same = match (left, right) {
                        (Val::Int(left), Val::Int(right)) => left == right,
                        (Val::Float(left), Val::Float(right)) => left == right,
                        (Val::Str(left), Val::Str(right)) => left == right,
                        (
                            Val::EnumValue {
                                owner: left_owner,
                                name: left_name,
                            },
                            Val::EnumValue {
                                owner: right_owner,
                                name: right_name,
                            },
                        ) => left_owner == right_owner && left_name == right_name,
                        (Val::Unknown, _) | (_, Val::Unknown) => false,
                        _ => false,
                    };
                    let taken = if op == 0xa5 { same } else { !same };
                    if taken {
                        next = branch_at(ins, at, current.a)?;
                    }
                }
                0xa7 => {
                    next = branch_at(ins, at, current.a)?;
                }
                0xa8 | 0xa9 => return Err("jsr/ret is not supported".to_string()),
                0xaa | 0xab => {
                    let key = stack.pop().unwrap_or(Val::Unknown);
                    if let Val::Int(key) = key {
                        let delta = current
                            .cases
                            .iter()
                            .find(|(value, _)| *value == key)
                            .map(|(_, delta)| *delta)
                            .unwrap_or(current.a);
                        next = branch_at(ins, at, delta)?;
                    }
                }
                0x2e..=0x35 => {
                    let index = stack.pop().unwrap_or(Val::Unknown);
                    let array = stack.pop().unwrap_or(Val::Unknown);
                    stack.push(match (array, index) {
                        (Val::SwitchMap(map), Val::Ordinal(name)) => {
                            Val::Int(map.get(&name).copied().unwrap_or(0))
                        }
                        _ => Val::Unknown,
                    });
                }
                0xac..=0xb0 => {
                    return Ok(Some(stack.pop().unwrap_or(Val::Unknown)));
                }
                0xb1 => return Ok(None),
                0xb2 => {
                    let value = self.getstatic(current.a)?;
                    stack.push(value);
                }
                0xb3 => {
                    let value = stack.pop().unwrap_or(Val::Unknown);
                    self.putstatic(current.a, value)?;
                }
                0xb4 => {
                    stack.pop();
                    stack.push(Val::Unknown);
                }
                0xb5 => {
                    stack.pop();
                    stack.pop();
                }
                0xb6 | 0xb7 => {
                    let (owner, name, descriptor) = self.member_ref(current.a)?;
                    let args = pop_args(stack, &descriptor);
                    let receiver = stack.pop().unwrap_or(Val::Unknown);
                    let result = self.invoke(&owner, &name, &descriptor, receiver, args)?;
                    if returns_value(&descriptor) {
                        stack.push(result);
                    }
                }
                0xb8 => {
                    let (owner, name, descriptor) = self.member_ref(current.a)?;
                    let args = pop_args(stack, &descriptor);
                    let result = self.invoke(&owner, &name, &descriptor, Val::Unknown, args)?;
                    if returns_value(&descriptor) {
                        stack.push(result);
                    }
                }
                0xb9 => {
                    let (owner, name, descriptor) = self.member_ref(current.a)?;
                    let args = pop_args(stack, &descriptor);
                    let receiver = stack.pop().unwrap_or(Val::Unknown);
                    let result = self.invoke(&owner, &name, &descriptor, receiver, args)?;
                    if returns_value(&descriptor) {
                        stack.push(result);
                    }
                }
                0xba => {
                    let value = self.invokedynamic(current.a, stack)?;
                    stack.push(value);
                }
                0xbb | 0xbd => stack.push(Val::Unknown),
                0xbc => stack.push(Val::Unknown),
                0xbe => {
                    stack.pop();
                    stack.push(Val::Int(0));
                }
                0xbf => return Err("athrow is not supported".to_string()),
                0xc0 => {}
                0xc1 => {
                    stack.pop();
                    stack.push(Val::Int(0));
                }
                0xc2 | 0xc3 => {}
                0xc4 => return Err("wide is not supported".to_string()),
                0xc5 => {
                    for _ in 0..current.a.max(0) as usize {
                        stack.pop();
                    }
                    stack.push(Val::Unknown);
                }
                0xc6 => {
                    let value = stack.pop().unwrap_or(Val::Unknown);
                    if matches!(value, Val::Unknown) {
                        next = branch_at(ins, at, current.a)?;
                    }
                }
                0xc7 => {
                    let value = stack.pop().unwrap_or(Val::Unknown);
                    if !matches!(value, Val::Unknown) {
                        next = branch_at(ins, at, current.a)?;
                    }
                }
                _ => {}
            }
            at = next;
        }
        Ok(None)
    }

    fn class(&mut self) -> Result<&ClassFile, String> {
        self.jar.class("net/minecraft/world/level/block/Blocks")
    }

    fn ldc(&mut self, index: i32) -> Result<Val, String> {
        let class = self.class()?;
        Ok(match class.constants.get(index as usize) {
            Some(Constant::String(reference)) => {
                let inner: usize = reference.trim_start_matches('#').parse().unwrap_or(0);
                Val::Str(classfile::utf8(&class.constants, inner))
            }
            Some(Constant::Integer(value)) => Val::Int(*value),
            Some(Constant::Float(value)) => Val::Float(*value),
            _ => Val::Unknown,
        })
    }

    fn member_ref(&mut self, index: i32) -> Result<(String, String, String), String> {
        let class = self.class()?;
        classfile::method_ref(&class.constants, index as usize)
            .or_else(|| classfile::field_ref(&class.constants, index as usize))
            .ok_or_else(|| format!("constant {index} is not a member reference"))
    }

    fn getstatic(&mut self, index: i32) -> Result<Val, String> {
        let (owner, name, descriptor) = {
            let class = self.class()?;
            match classfile::field_ref(&class.constants, index as usize) {
                Some(reference) => reference,
                None => return Ok(Val::Unknown),
            }
        };
        let owner_simple = owner.rsplit('/').next().unwrap_or(&owner).to_string();
        Ok(if owner_simple == "Blocks" {
            if descriptor == "Lnet/minecraft/world/level/block/Block;" {
                let props = self.scan.fields.get(&name).cloned();
                Val::Block {
                    field: Some(name),
                    props,
                }
            } else if descriptor.contains("ColorCollection")
                || descriptor.contains("WeatheringCopperCollection")
            {
                match self.scan.collections.get(&name) {
                    Some(collection) => Val::Collection(collection.clone()),
                    None => Val::Object("Collection".to_string()),
                }
            } else {
                Val::Object("BlockItemId".to_string())
            }
        } else if owner_simple == "EntityTypes" {
            Val::EntityType(name.to_lowercase())
        } else if owner_simple == "BlockTags" {
            Val::Tag(format!("minecraft:{}", lower_snake(&name)))
        } else if owner_simple == "ColorCollection" && name == "VALUES" {
            Val::Object("DyeColorCollection".to_string())
        } else if name.starts_with("$SwitchMap") {
            match self.switch_map(&owner) {
                Ok(map) => Val::SwitchMap(map),
                Err(_) => Val::Unknown,
            }
        } else if owner_simple == "CandleBlock" && name == "LIGHT_EMISSION" {
            Val::Light(LightFun::Zero)
        } else if owner_simple == "LightBlock" && name == "LIGHT_EMISSION" {
            Val::Light(LightFun::Property("level"))
        } else if is_property_owner(&owner_simple) {
            Val::Property(name)
        } else {
            Val::Unknown
        })
    }

    /// Rebuild a compiler-generated `$SwitchMap$...$Enum` table: enum constant
    /// name -> case label. Its initialiser is a run of
    /// `getstatic table; getstatic Enum.CONST; ordinal; iconst N; iastore`.
    fn switch_map(&mut self, owner: &str) -> Result<BTreeMap<String, i32>, String> {
        let code = {
            let class = self.jar.class(owner)?;
            class
                .method("<clinit>")
                .and_then(|method| method.code.as_ref())
                .map(|code| code.bytes.clone())
                .ok_or_else(|| format!("{owner}.<clinit> has no code"))?
        };
        let ins = decode(&code)?;
        let mut map = BTreeMap::new();
        let mut pending_enum = None;
        let mut pending_value = None;
        for instruction in &ins {
            match instruction.op {
                0xb2 => {
                    let class = self.jar.class(owner)?;
                    if let Some((_, name, descriptor)) =
                        classfile::field_ref(&class.constants, instruction.a as usize)
                    {
                        if !descriptor.starts_with('[') {
                            pending_enum = Some(name);
                        }
                    }
                }
                0x02..=0x08 => pending_value = Some(instruction.op as i32 - 3),
                0x10 | 0x11 => pending_value = Some(instruction.a),
                0x4f => {
                    if let (Some(name), Some(value)) = (pending_enum.take(), pending_value.take()) {
                        map.insert(name, value);
                    }
                }
                _ => {}
            }
        }
        Ok(map)
    }

    fn putstatic(&mut self, index: i32, value: Val) -> Result<(), String> {
        let (owner, name, descriptor) = {
            let class = self.class()?;
            match classfile::field_ref(&class.constants, index as usize) {
                Some(reference) => reference,
                None => return Ok(()),
            }
        };
        if owner.rsplit('/').next() != Some("Blocks") {
            return Ok(());
        }
        match value {
            Val::Block {
                props: Some(props), ..
            } if descriptor == "Lnet/minecraft/world/level/block/Block;" => {
                self.scan.fields.insert(name, props);
            }
            Val::Collection(collection) => {
                self.scan.collections.insert(name, collection);
            }
            _ => {}
        }
        Ok(())
    }

    fn invoke(
        &mut self,
        owner: &str,
        name: &str,
        descriptor: &str,
        receiver: Val,
        args: Vec<Val>,
    ) -> Result<Val, String> {
        let owner_simple = owner.rsplit('/').next().unwrap_or(owner);
        if name == "ordinal" {
            if let Val::EnumValue { name, .. } = &receiver {
                return Ok(Val::Ordinal(name.to_uppercase()));
            }
        }
        if owner_simple == "Blocks" {
            return self.invoke_blocks(name, descriptor, args);
        }
        if owner_simple == "BlockBehaviour$Properties" {
            return Ok(self.properties_call(name, receiver, args));
        }
        if owner_simple == "ColorCollection" {
            return self.color_collection_call(name, receiver, args);
        }
        if owner_simple == "WeatheringCopperCollection" {
            return self.copper_collection_call(name, args);
        }
        if (name == "apply" || name == "test" || name == "applyAsInt") && args.len() < 4 {
            if let Val::Func(function) = receiver {
                return self.call_func(&function, args);
            }
        }
        if owner_simple == "Boolean" || owner_simple == "Integer" {
            return Ok(args.into_iter().next().unwrap_or(Val::Unknown));
        }
        Ok(Val::Unknown)
    }

    fn invoke_blocks(
        &mut self,
        name: &str,
        descriptor: &str,
        args: Vec<Val>,
    ) -> Result<Val, String> {
        match name {
            "register" => {
                let props = args
                    .iter()
                    .rev()
                    .find_map(|value| match value {
                        Val::Props(props) => Some(props.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                self.record(props.clone());
                Ok(Val::Block {
                    field: None,
                    props: Some(props),
                })
            }
            "litBlockEmission" => {
                let value = match args.first() {
                    Some(Val::Int(value)) => *value,
                    _ => 15,
                };
                Ok(Val::Light(LightFun::Lit(value)))
            }
            "never" | "always" | "ocelotOrParrot" => Ok(Val::Func(Func {
                owner: "net/minecraft/world/level/block/Blocks".to_string(),
                name: name.to_string(),
                descriptor: descriptor.to_string(),
                captured: Vec::new(),
            })),
            _ => self.eval_blocks_method(name, descriptor, args),
        }
    }

    fn record(&mut self, props: Props) {
        self.scan.registrations.push(props);
    }

    fn eval_blocks_method(
        &mut self,
        name: &str,
        descriptor: &str,
        args: Vec<Val>,
    ) -> Result<Val, String> {
        if self.depth > 32 {
            return Ok(Val::Unknown);
        }
        let ins = {
            let class = self.class()?;
            let Some(method) = class
                .methods
                .iter()
                .find(|method| method.name == name && method.descriptor == descriptor)
            else {
                return Ok(Val::Unknown);
            };
            let Some(code) = method.code.as_ref() else {
                return Ok(Val::Unknown);
            };
            decode(&code.bytes)?
        };
        self.depth += 1;
        let mut locals = args;
        let mut stack = Vec::new();
        let result = self.exec(&ins, &mut stack, &mut locals).ok().flatten();
        self.depth -= 1;
        Ok(result.unwrap_or(Val::Unknown))
    }

    fn call_func(&mut self, function: &Func, args: Vec<Val>) -> Result<Val, String> {
        if function.owner.rsplit('/').next() != Some("Blocks") || self.depth > 32 {
            return Ok(Val::Unknown);
        }
        let mut locals = function.captured.clone();
        locals.extend(args);
        let ins = {
            let class = self.class()?;
            let Some(method) = class.methods.iter().find(|method| {
                method.name == function.name && method.descriptor == function.descriptor
            }) else {
                return Ok(Val::Unknown);
            };
            let Some(code) = method.code.as_ref() else {
                return Ok(Val::Unknown);
            };
            decode(&code.bytes)?
        };
        self.depth += 1;
        let mut stack = Vec::new();
        let result = self.exec(&ins, &mut stack, &mut locals).ok().flatten();
        self.depth -= 1;
        Ok(result.unwrap_or(Val::Unknown))
    }

    fn properties_call(&mut self, name: &str, receiver: Val, args: Vec<Val>) -> Val {
        let mut props = match receiver {
            Val::Props(props) => props,
            _ => Props::default(),
        };
        match name {
            "isValidSpawn" => {
                if let Some(Val::Func(function)) = args.first() {
                    props.spawn = Some(self.classify_spawn(function));
                }
            }
            "isRedstoneConductor" => {
                if let Some(Val::Func(function)) = args.first() {
                    props.redstone = Some(self.classify_redstone(function));
                }
            }
            "lightLevel" => {
                let light = match args.first() {
                    Some(Val::Light(kind)) => *kind,
                    Some(Val::Func(function)) => self.classify_light(function),
                    _ => LightFun::Zero,
                };
                props.light = Some(light);
            }
            "forceSolidOn" => props.force_solid_on = Some(true),
            "forceSolidOff" => props.force_solid_off = Some(true),
            "noCollision" => {
                props.has_collision = Some(false);
                // `Properties.noCollision()` clears `canOcclude` as well.
                props.can_occlude = Some(false);
            }
            "noOcclusion" => props.can_occlude = Some(false),
            "dynamicShape" => props.dynamic_shape = Some(true),
            "ofLegacyCopy" | "ofFullCopy" => {
                let kind = if name == "ofLegacyCopy" {
                    CopyKind::Legacy
                } else {
                    CopyKind::Full
                };
                let source = match args.first() {
                    Some(Val::Block {
                        field: Some(field), ..
                    }) => Some(CopySource::Field(field.clone())),
                    Some(Val::Block {
                        props: Some(props), ..
                    }) => Some(CopySource::Inline(Box::new(props.clone()))),
                    _ => None,
                };
                if let Some(source) = source {
                    props = Props {
                        copy: Some((source, kind)),
                        ..Props::default()
                    };
                }
            }
            _ => {}
        }
        Val::Props(props)
    }

    fn classify_spawn(&mut self, function: &Func) -> SpawnPred {
        match function.name.as_str() {
            "never" => return SpawnPred::Never,
            "always" => return SpawnPred::Always,
            "ocelotOrParrot" => return SpawnPred::OcelotOrParrot,
            _ => {}
        }
        if function.owner.rsplit('/').next() != Some("Blocks") {
            return SpawnPred::Unknown;
        }
        let Some(body) = self.function_body(function) else {
            return SpawnPred::Unknown;
        };
        let refs = self.function_refs(&body);
        if refs
            .iter()
            .any(|reference| reference == "EntityTypes.POLAR_BEAR")
        {
            return SpawnPred::PolarBear;
        }
        if refs
            .iter()
            .any(|reference| reference == "EntityTypes.OCELOT" || reference == "EntityTypes.PARROT")
        {
            return SpawnPred::OcelotOrParrot;
        }
        if refs
            .iter()
            .any(|reference| reference == "EntityTypes.ZOMBIE")
        {
            return SpawnPred::ZombieOnly;
        }
        if refs
            .iter()
            .any(|reference| reference.ends_with(".fireImmune"))
        {
            return SpawnPred::FireImmune;
        }
        if refs
            .iter()
            .any(|reference| reference.ends_with(".isFaceSturdy"))
            && refs
                .iter()
                .any(|reference| reference.ends_with(".getLightEmission"))
        {
            return SpawnPred::Default;
        }
        for (name, descriptor) in self.called_blocks(&body) {
            let nested = Func::blocks(&name, &descriptor);
            let classified = self.classify_spawn(&nested);
            if classified != SpawnPred::Unknown {
                return classified;
            }
        }
        if body
            .iter()
            .all(|instruction| matches!(instruction.op, 0x02 | 0x03))
        {
            return if body.iter().any(|instruction| instruction.op == 0x03) {
                SpawnPred::Always
            } else {
                SpawnPred::Never
            };
        }
        SpawnPred::Unknown
    }

    fn classify_redstone(&mut self, function: &Func) -> RedstonePred {
        match function.name.as_str() {
            "never" => return RedstonePred::Never,
            "always" => return RedstonePred::Always,
            _ => {}
        }
        if function.owner.rsplit('/').next() != Some("Blocks") {
            return RedstonePred::Unknown;
        }
        let Some(body) = self.function_body(function) else {
            return RedstonePred::Unknown;
        };
        let refs = self.function_refs(&body);
        if refs
            .iter()
            .any(|reference| reference.ends_with(".isCollisionShapeFullBlock"))
        {
            return RedstonePred::Default;
        }
        for (name, descriptor) in self.called_blocks(&body) {
            let nested = Func::blocks(&name, &descriptor);
            let classified = self.classify_redstone(&nested);
            if classified != RedstonePred::Unknown {
                return classified;
            }
        }
        RedstonePred::Unknown
    }

    fn classify_light(&mut self, function: &Func) -> LightFun {
        if function.name.starts_with("lambda$litBlockEmission$") {
            let value = function
                .captured
                .iter()
                .find_map(|value| match value {
                    Val::Int(value) => Some(*value),
                    _ => None,
                })
                .unwrap_or(15);
            return LightFun::Lit(value);
        }
        let Some(body) = self.function_body(function) else {
            return LightFun::Zero;
        };
        let refs = self.function_refs(&body);
        if refs
            .iter()
            .any(|reference| reference.contains("getScaledChargeLevel"))
        {
            return LightFun::RespawnAnchor;
        }
        if refs.iter().any(|reference| {
            reference.contains("SeaPickleBlock.isDead") || reference.ends_with(".PICKLES")
        }) {
            return LightFun::SeaPickle;
        }
        if refs.iter().any(|reference| {
            reference.contains("VaultBlock.STATE") || reference.contains("TrialSpawnerBlock.STATE")
        }) {
            return LightFun::Zero;
        }
        if refs.iter().any(|reference| reference.ends_with(".LIT")) {
            return LightFun::Lit(last_int(&body));
        }
        if refs.iter().any(|reference| reference.ends_with(".LEVEL")) {
            return LightFun::Property("level");
        }
        if refs.is_empty() {
            return LightFun::Constant(last_int(&body));
        }
        LightFun::Zero
    }

    fn function_body(&mut self, function: &Func) -> Option<Vec<Ins>> {
        let class = self.class().ok()?;
        let method = class.methods.iter().find(|method| {
            method.name == function.name && method.descriptor == function.descriptor
        })?;
        let code = method.code.as_ref()?;
        decode(&code.bytes).ok()
    }

    fn function_refs(&mut self, body: &[Ins]) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let Ok(class) = self.class() else {
            return out;
        };
        for instruction in body {
            let reference = match instruction.op {
                0xb2..=0xb5 => classfile::field_ref(&class.constants, instruction.a as usize),
                0xb6..=0xb9 => classfile::method_ref(&class.constants, instruction.a as usize),
                _ => None,
            };
            if let Some((owner, name, _)) = reference {
                out.insert(format!(
                    "{}.{}",
                    owner.rsplit('/').next().unwrap_or(&owner),
                    name
                ));
            }
        }
        out
    }

    fn called_blocks(&mut self, body: &[Ins]) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let Ok(class) = self.class() else {
            return out;
        };
        for instruction in body {
            if instruction.op == 0xb8 {
                if let Some((owner, name, descriptor)) =
                    classfile::method_ref(&class.constants, instruction.a as usize)
                {
                    if owner.rsplit('/').next() == Some("Blocks") {
                        out.push((name, descriptor));
                    }
                }
            }
        }
        out
    }

    fn invokedynamic(&mut self, index: i32, stack: &mut Vec<Val>) -> Result<Val, String> {
        let class = self.class()?;
        let Some(Constant::InvokeDynamic {
            name, bootstrap, ..
        }) = class.constants.get(index as usize)
        else {
            return Ok(Val::Unknown);
        };
        let bootstrap = *bootstrap;
        let name_index: usize = name.trim_start_matches('#').parse().unwrap_or(0);
        let descriptor = match class.constants.get(name_index) {
            Some(Constant::NameAndType { kind, .. }) => {
                let inner: usize = kind.trim_start_matches('#').parse().unwrap_or(0);
                classfile::utf8(&class.constants, inner)
            }
            _ => return Ok(Val::Unknown),
        };
        let arguments = class
            .bootstraps
            .get(bootstrap)
            .map(|entry| entry.arguments.clone())
            .unwrap_or_default();
        let mut captured = pop_args(stack, &descriptor);
        captured.reverse();
        let mut target = None;
        for argument in &arguments {
            if let Some(Constant::MethodHandle { reference }) = class.constants.get(*argument) {
                if let Some(reference) = classfile::method_ref(&class.constants, *reference) {
                    target = Some(reference);
                    break;
                }
            }
        }
        let Some((owner, name, descriptor)) = target else {
            return Ok(Val::Unknown);
        };
        Ok(Val::Func(Func {
            owner,
            name,
            descriptor,
            captured,
        }))
    }

    fn color_collection_call(
        &mut self,
        name: &str,
        receiver: Val,
        args: Vec<Val>,
    ) -> Result<Val, String> {
        match name {
            "registerBlocks" => {
                let props_fn = args.get(3).cloned().unwrap_or(Val::Unknown);
                let mut collection = Vec::with_capacity(16);
                for index in 0..16 {
                    let props = self.collection_props(
                        &props_fn,
                        Val::EnumValue {
                            owner: "DyeColor".to_string(),
                            name: dye_color_name(index).to_string(),
                        },
                    );
                    self.record(props.clone());
                    collection.push(props);
                }
                Ok(Val::Collection(collection))
            }
            "zipMap" => {
                let function = args.get(2).cloned().unwrap_or(Val::Unknown);
                let mut collection = Vec::with_capacity(16);
                for index in 0..16 {
                    let result = self.eval_value_function(
                        &function,
                        vec![
                            Val::EnumValue {
                                owner: "DyeColor".to_string(),
                                name: dye_color_name(index).to_string(),
                            },
                            Val::Object("BlockItemId".to_string()),
                        ],
                    );
                    let props = match &result {
                        Val::Block {
                            props: Some(props), ..
                        } => props.clone(),
                        _ => self.scan.registrations.last().cloned().unwrap_or_default(),
                    };
                    collection.push(props);
                }
                Ok(Val::Collection(collection))
            }
            "pick" | "get" => {
                let index = match args.first() {
                    Some(Val::EnumValue { name, .. }) => dye_color_index(name),
                    Some(Val::Int(value)) => (*value).max(0) as usize,
                    _ => 0,
                };
                if let Val::Collection(collection) = receiver {
                    if let Some(props) = collection.get(index) {
                        return Ok(Val::Block {
                            field: None,
                            props: Some(props.clone()),
                        });
                    }
                }
                Ok(Val::Unknown)
            }
            _ => Ok(Val::Unknown),
        }
    }

    fn copper_collection_call(&mut self, name: &str, args: Vec<Val>) -> Result<Val, String> {
        match name {
            "registerBlocks" => {
                let props_fn = args.get(4).cloned().unwrap_or(Val::Unknown);
                if self.scan.registrations.len() > 900 {
                    eprintln!(
                        "copper registerBlocks start={} props_fn={:?}",
                        self.scan.registrations.len(),
                        props_fn
                    );
                }
                let mut collection = Vec::with_capacity(8);
                let start = self.scan.registrations.len();
                for index in 0..8 {
                    let state = self
                        .registry
                        .get(start + index)
                        .map(|block| weather_state_of(&block.id))
                        .unwrap_or("unaffected");
                    let props = self.collection_props(
                        &props_fn,
                        Val::EnumValue {
                            owner: "WeatherState".to_string(),
                            name: state.to_string(),
                        },
                    );
                    self.record(props.clone());
                    collection.push(props);
                }
                Ok(Val::Collection(collection))
            }
            _ => Ok(Val::Unknown),
        }
    }

    fn collection_props(&mut self, function: &Val, argument: Val) -> Props {
        match self.eval_value_function(function, vec![argument]) {
            Val::Props(props) => props,
            _ => Props::default(),
        }
    }

    fn eval_value_function(&mut self, function: &Val, args: Vec<Val>) -> Val {
        match function {
            Val::Func(func) => self.call_func(func, args).unwrap_or(Val::Unknown),
            _ => Val::Unknown,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn set_local(locals: &mut Vec<Val>, index: usize, value: Val) {
    if locals.len() <= index {
        locals.resize(index + 1, Val::Unknown);
    }
    locals[index] = value;
}

fn pop_args(stack: &mut Vec<Val>, descriptor: &str) -> Vec<Val> {
    let count = classfile::descriptor_types(descriptor)
        .len()
        .saturating_sub(1);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(stack.pop().unwrap_or(Val::Unknown));
    }
    out.reverse();
    out
}

fn returns_value(descriptor: &str) -> bool {
    classfile::descriptor_types(descriptor)
        .last()
        .map(|kind| kind != "V")
        .unwrap_or(false)
}

fn arithmetic(op: u8, left: Val, right: Val) -> Val {
    match (left, right) {
        (Val::Int(left), Val::Int(right)) => match op {
            0x60 => Val::Int(left + right),
            0x64 => Val::Int(left - right),
            0x68 => Val::Int(left * right),
            0x6c => Val::Int(if right == 0 { 0 } else { left / right }),
            0x70 => Val::Int(if right == 0 { 0 } else { left % right }),
            0x74 => Val::Int(-left),
            _ => Val::Unknown,
        },
        (Val::Float(left), Val::Float(right)) => match op {
            0x62 => Val::Float(left + right),
            0x66 => Val::Float(left - right),
            0x6a => Val::Float(left * right),
            0x6e => Val::Float(left / right),
            0x76 => Val::Float(-left),
            _ => Val::Unknown,
        },
        _ => Val::Unknown,
    }
}

fn convert(op: u8, value: Val) -> Val {
    match (op, value) {
        (0x86, Val::Int(value)) => Val::Float(value as f32),
        (0x8b, Val::Float(value)) => Val::Int(value as i32),
        _ => Val::Unknown,
    }
}

fn last_int(body: &[Ins]) -> i32 {
    let mut value = 0;
    for instruction in body {
        match instruction.op {
            0x02..=0x08 => value = instruction.op as i32 - 3,
            0x10 | 0x11 => value = instruction.a,
            _ => {}
        }
    }
    value
}

fn is_property_owner(owner: &str) -> bool {
    owner == "BlockStateProperties" || owner.ends_with("Property") || owner.ends_with("Properties")
}

fn lower_snake(name: &str) -> String {
    let mut out = String::new();
    for character in name.chars() {
        if character.is_ascii_uppercase() {
            if !out.is_empty() {
                out.push('_');
            }
            out.push(character.to_ascii_lowercase());
        } else {
            out.push(character);
        }
    }
    out
}

fn dye_color_name(index: usize) -> &'static str {
    const NAMES: [&str; 16] = [
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
    NAMES.get(index).copied().unwrap_or("white")
}

fn dye_color_index(name: &str) -> usize {
    (0..16)
        .position(|index| dye_color_name(index) == name)
        .unwrap_or(0)
}

fn weather_state_of(block_id: &str) -> &'static str {
    let id = block_id.trim_start_matches("minecraft:");
    if id.contains("oxidized") {
        "oxidized"
    } else if id.contains("weathered") {
        "weathered"
    } else if id.contains("exposed") {
        "exposed"
    } else {
        "unaffected"
    }
}

// ---------------------------------------------------------------------------
// Scan entry point and jar access
// ---------------------------------------------------------------------------

fn scan_blocks(jar_path: &Path, registry: &[blockmod::Block]) -> Result<Scan, String> {
    let mut jar = Jar::open(jar_path)?;
    let dynamic = dynamic_classes(&mut jar);
    let interp = Interp::new(&mut jar, registry);
    let mut scan = interp.run()?;
    scan.dynamic = dynamic;
    let overrides = scan
        .registrations
        .iter()
        .filter(|props| {
            props.spawn.is_some()
                || props.redstone.is_some()
                || props.light.is_some()
                || props.force_solid_on.is_some()
                || props.force_solid_off.is_some()
                || props.can_occlude.is_some()
                || props.has_collision.is_some()
                || props.copy.is_some()
        })
        .count();
    scan.notes.push(format!(
        "{} registered blocks, {overrides} with properties overrides, {} property fields, {} collections",
        scan.registrations.len(),
        scan.fields.len(),
        scan.collections.len()
    ));
    Ok(scan)
}

struct Jar {
    archive: zip::ZipArchive<fs::File>,
    classes: BTreeMap<String, ClassFile>,
}

/// Every block class whose `hasDynamicShape()` returns a constant `true`.
///
/// The cached collision shape of such a state is `null`, which is what makes
/// `isSolid` false (or, for the shulker box, `forceSolidOn` win first).
fn dynamic_classes(jar: &mut Jar) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for name in jar.class_names() {
        let Ok(class) = jar.class(&name) else {
            continue;
        };
        let Some(method) = class
            .methods
            .iter()
            .find(|method| method.name == "hasDynamicShape" && method.descriptor == "()Z")
        else {
            continue;
        };
        let Some(code) = method.code.as_ref() else {
            continue;
        };
        let Ok(ins) = decode(&code.bytes) else {
            continue;
        };
        let returns_true = ins.iter().any(|instruction| instruction.op == 0x03);
        let calls = ins
            .iter()
            .any(|instruction| matches!(instruction.op, 0xb6..=0xb9));
        if returns_true && !calls {
            out.insert(name.rsplit('/').next().unwrap_or(&name).to_string());
        }
    }
    out
}

impl Jar {
    fn open(path: &Path) -> Result<Jar, String> {
        let file = fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let archive =
            zip::ZipArchive::new(file).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(Jar {
            archive,
            classes: BTreeMap::new(),
        })
    }

    fn class(&mut self, internal: &str) -> Result<&ClassFile, String> {
        if !self.classes.contains_key(internal) {
            use std::io::Read;
            let file_name = format!("{internal}.class");
            let mut entry = self
                .archive
                .by_name(&file_name)
                .map_err(|error| format!("{file_name}: {error}"))?;
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .map_err(|error| error.to_string())?;
            let class =
                classfile::parse(&data).map_err(|error| format!("{file_name}: {error:?}"))?;
            self.classes.insert(internal.to_string(), class);
        }
        Ok(self.classes.get(internal).expect("class was just read"))
    }

    /// Internal names of every block class in the jar (no inner classes).
    fn class_names(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        for index in 0..self.archive.len() {
            let Ok(entry) = self.archive.by_index(index) else {
                continue;
            };
            if entry.is_dir() {
                continue;
            }
            let Ok(name) = entry.name() else {
                continue;
            };
            let name = name.to_string();
            if !name.starts_with("net/minecraft/world/level/block/") {
                continue;
            }
            let Some(internal) = name.strip_suffix(".class") else {
                continue;
            };
            if internal.contains('$') {
                continue;
            }
            out.push(internal.to_string());
        }
        out
    }
}

// ---------------------------------------------------------------------------
// State values and shape predicates
// ---------------------------------------------------------------------------

fn state_values(block: &statemod::BlockStates, offset: usize) -> BTreeMap<String, String> {
    let sorted = block.sorted();
    let mut out = BTreeMap::new();
    let mut remainder = offset;
    for property in sorted.iter().rev() {
        let count = property.values.len().max(1);
        let slot = remainder % count;
        remainder /= count;
        out.insert(property.name.clone(), property.values[slot].clone());
    }
    out
}

fn value_of<'a>(values: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    values.get(name).map(String::as_str)
}

fn int_of(values: &BTreeMap<String, String>, name: &str) -> i32 {
    values
        .get(name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn shape_kind(shape: Option<&geometry::Shape>) -> &'static str {
    match shape {
        None => "none",
        Some(shape) if shape_is_empty(shape) => "empty",
        Some(shape) if shape_is_full(shape) => "full",
        Some(_) => "partial",
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
    geometry::Shape {
        x: vec![0.0],
        y: vec![0.0],
        z: vec![0.0],
        canonical: 1,
        cube: 0,
        cube_axes: 0,
        discrete_min: [0, 0, 0],
        discrete_max: [0, 0, 0],
        occupied: Vec::new(),
    }
}

fn shape_is_empty(shape: &geometry::Shape) -> bool {
    shape.occupied.iter().all(|value| *value == 0)
}

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
    (0..ys).all(|y| {
        (0..zs).all(|z| {
            (0..xs).all(|x| {
                let index = (x * ys + y) * zs + z;
                shape.occupied.get(index).copied().unwrap_or(0) != 0
            })
        })
    })
}

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
    if shape.y.len() == 2 && shape.y[0] == 0.0 && shape.y[1] == 1.0 {
        return false;
    }
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
    // `VoxelShape.bounds()` uses `min`/`max`, which index the coordinate list
    // with the discrete `firstFull`/`lastFull` cells, not the raw coordinate
    // range (a cube-shaped grid can carry unused cells).
    let coords = [&shape.x, &shape.y, &shape.z];
    let mut out = [0.0f64; 6];
    for axis in 0..3 {
        let min = shape.discrete_min[axis].max(0) as usize;
        let max = shape.discrete_max[axis].max(0) as usize;
        out[axis] = coords[axis].get(min).copied().unwrap_or(0.0);
        out[axis + 3] = coords[axis].get(max).copied().unwrap_or(0.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_spawn_predicate_is_the_sturdy_top_and_light_rule() {
        assert!(SpawnPred::Default.spawns(Entity::Zombie, true, 0));
        assert!(!SpawnPred::Default.spawns(Entity::Zombie, false, 0));
        assert!(!SpawnPred::Default.spawns(Entity::Zombie, true, 14));
        assert!(SpawnPred::Default.spawns(Entity::Ocelot, true, 13));
    }

    #[test]
    fn entity_specific_predicates_answer_the_three_probes() {
        assert!(SpawnPred::PolarBear.spawns(Entity::PolarBear, false, 15));
        assert!(!SpawnPred::PolarBear.spawns(Entity::Zombie, true, 0));
        assert!(SpawnPred::OcelotOrParrot.spawns(Entity::Ocelot, false, 15));
        assert!(!SpawnPred::OcelotOrParrot.spawns(Entity::Zombie, true, 0));
        assert!(SpawnPred::ZombieOnly.spawns(Entity::Zombie, false, 15));
        assert!(!SpawnPred::ZombieOnly.spawns(Entity::PolarBear, true, 0));
    }

    #[test]
    fn light_functions_drive_the_spawn_rule() {
        let values = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect::<BTreeMap<_, _>>()
        };
        assert_eq!(light_of(&Some(LightFun::Constant(15)), &values(&[])), 15);
        assert_eq!(
            light_of(&Some(LightFun::Lit(15)), &values(&[("lit", "true")])),
            15
        );
        assert_eq!(
            light_of(&Some(LightFun::Lit(15)), &values(&[("lit", "false")])),
            0
        );
        assert_eq!(
            light_of(&Some(LightFun::RespawnAnchor), &values(&[("charges", "4")])),
            15
        );
        assert_eq!(
            light_of(&Some(LightFun::RespawnAnchor), &values(&[("charges", "3")])),
            11
        );
        assert_eq!(
            light_of(
                &Some(LightFun::Property("level")),
                &values(&[("level", "7")])
            ),
            7
        );
    }

    #[test]
    fn a_full_copy_inherits_the_predicates_but_a_legacy_copy_does_not() {
        let base = Props {
            spawn: Some(SpawnPred::Never),
            redstone: Some(RedstonePred::Never),
            can_occlude: Some(false),
            ..Props::default()
        };
        let full = resolve_props(
            &Props {
                copy: Some((CopySource::Inline(Box::new(base.clone())), CopyKind::Full)),
                ..Props::default()
            },
            &BTreeMap::new(),
            0,
        );
        assert_eq!(full.spawn, Some(SpawnPred::Never));
        assert_eq!(full.redstone, Some(RedstonePred::Never));
        assert_eq!(full.can_occlude, Some(false));
        let legacy = resolve_props(
            &Props {
                copy: Some((CopySource::Inline(Box::new(base)), CopyKind::Legacy)),
                ..Props::default()
            },
            &BTreeMap::new(),
            0,
        );
        assert_eq!(legacy.spawn, None);
        assert_eq!(legacy.redstone, None);
        assert_eq!(legacy.can_occlude, Some(false));
    }

    #[test]
    fn the_shape_predicates_recognise_a_full_cube_and_its_top_face() {
        // `shape_is_full`/`shape_face_full_up` are copied from the shape
        // predicates' semantics; a 16x16x16 box is both.
        let block = geometry::Shape {
            x: vec![0.0, 1.0],
            y: vec![0.0, 1.0],
            z: vec![0.0, 1.0],
            canonical: 2,
            cube: 1,
            cube_axes: 7,
            discrete_min: [0, 0, 0],
            discrete_max: [1, 1, 1],
            occupied: vec![1],
        };
        assert!(shape_is_full(&block));
        assert!(shape_face_full_up(&block));
        let slab = geometry::Shape {
            x: vec![0.0, 1.0],
            y: vec![0.0, 0.5, 1.0],
            z: vec![0.0, 1.0],
            canonical: 0,
            cube: 1,
            cube_axes: 7,
            discrete_min: [0, 0, 0],
            discrete_max: [1, 1, 1],
            occupied: vec![1, 0],
        };
        assert!(!shape_is_full(&slab));
        assert!(!shape_face_full_up(&slab));
    }
}
