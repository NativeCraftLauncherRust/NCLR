//! Rebuild Horizon's client/render tables from the player's own `client.jar`,
//! without reading `ship.hzr3`.
//!
//! `horizon-prepare` runs Java oracles against the real 26.3 client jar and
//! writes three tables this module targets:
//!
//! * `mob-data.json` — `VanillaMobDataExporter`: per-block-state path types,
//!   per-entity-type spawn placements, and the client's keyframe animation
//!   definitions.
//! * `entity-models.json` — `VanillaEntityModelExporter`: every baked
//!   `LayerDefinitions` root as a part tree/polygon list.
//! * `render-definitions.json` — `VanillaFaceVisibilityOracle`: per-block
//!   skip-rendering rule, occlusion flags and the block's state-id range.
//!
//! ## What is implemented here
//!
//! Two of `mob-data.json`'s three sections are bytecode facts, not simulation,
//! so they can be rebuilt from the jar with a purpose-built JVM reader:
//!
//! * `animations`: every `static final AnimationDefinition` field of every
//!   top-level class in `net/minecraft/client/animation/definitions/`. Those
//!   initialisers are straight-line JVM code that only calls
//!   `AnimationDefinition$Builder`, `AnimationChannel`, `Keyframe` and the
//!   `KeyframeAnimations` vector helpers. This module interprets exactly that
//!   shape and fails loudly (class + method + pc) on anything else.
//! * `placements`: `SpawnPlacements.<clinit>` registers every mob's placement
//!   and heightmap in one straight-line block; the entity-type registry order
//!   comes from `EntityTypes.<clinit>` (`register(EntityTypeIds.X, builder)` +
//!   `putstatic FIELD`).
//!
//! ## What is blocked (see [`blocked_tables`])
//!
//! * `mob-data.json#states` — `WalkNodeEvaluator.getPathTypeFromState`, the
//!   per-state flags and collision heights are vanilla *code*, evaluated once
//!   per entry of `Block.BLOCK_STATE_REGISTRY`. Rebuilding it needs the whole
//!   block-state registry (block order, each block's property cardinalities and
//!   value orders, and each block's default-state override) plus a port of the
//!   node evaluator's instanceof/tag ladder. The block-state id table itself is
//!   still missing from route3 (see `registries.rs`'s note on `blocks`).
//! * `render-definitions.json` — every row carries the block's state-id range
//!   and default-state id, i.e. `Block.getId(state)`. Those ids are assigned
//!   when the block registry is built from code (`Blocks.<clinit>`,
//!   `createBlockStateDefinition`, `registerDefaultState`), not from data; 746
//!   of 1,286 blocks override their default state in their constructor. The
//!   table cannot be emitted without the state-id table.
//! * `entity-models.json` — `LayerDefinitions.createRoots()` evaluates ~165
//!   model classes to build `MeshDefinition`s (with `CubeDeformation`,
//!   `MeshTransformer`/`ArmorModelSet.map` lambdas), then `bakeRoot()` runs
//!   `ModelPart.Cube`'s polygon and texture-UV generation. That is a port of
//!   vanilla's model-baking code, not an extraction of constants.
//!
//! Public entry points:
//! * [`export_mob_data`] -> [`MobData`] (implemented sections only)
//! * [`write_mob_data`]   -> `mob-data.json`
//! * [`blocked_tables`]   -> the exact reason each absent section/table is absent
//!
//! The test harness in `tests/render.rs` includes this file by path because the
//! crate's `main.rs` is a binary and must not be edited.

// Until `main.rs` wires this module in as a CLI command, the only compiled
// consumer is the integration test, so parts of the reader and the convenience
// write helpers have no in-tree caller yet.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

use serde_json::{Map, Number, Value};

#[path = "../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;

use classfile::{ClassFile, Constant};

const ENTITY_TYPE_IDS: &str = "net/minecraft/world/entity/EntityTypeIds";
const ENTITY_TYPES: &str = "net/minecraft/world/entity/EntityTypes";
const ENTITY_TYPE_BUILDER: &str = "net/minecraft/world/entity/EntityType$Builder";
const SPAWN_PLACEMENTS: &str = "net/minecraft/world/entity/SpawnPlacements";
const SPAWN_PLACEMENT_TYPES: &str = "net/minecraft/world/entity/SpawnPlacementTypes";
const HEIGHTMAP_TYPES: &str = "net/minecraft/world/level/levelgen/Heightmap$Types";

const ANIMATION_DEFINITIONS: &str = "net/minecraft/client/animation/definitions/";
const ANIMATION_DEFINITION: &str = "net/minecraft/client/animation/AnimationDefinition";
const ANIMATION_BUILDER: &str = "net/minecraft/client/animation/AnimationDefinition$Builder";
const ANIMATION_CHANNEL: &str = "net/minecraft/client/animation/AnimationChannel";
const ANIMATION_TARGETS: &str = "net/minecraft/client/animation/AnimationChannel$Targets";
const ANIMATION_INTERPOLATIONS: &str =
    "net/minecraft/client/animation/AnimationChannel$Interpolations";
const KEYFRAME: &str = "net/minecraft/client/animation/Keyframe";
const KEYFRAME_ANIMATIONS: &str = "net/minecraft/client/animation/KeyframeAnimations";
const VECTOR3F: &str = "org/joml/Vector3f";

/// `KeyframeAnimations.degreeVec`'s degrees-to-radians factor, as the float the
/// class file holds (`0.017453292f`).
const DEGREES_TO_RADIANS: f32 = 0.017_453_292_f32;

/// The sections/tables this module cannot rebuild from the client jar alone,
/// with the reason each one is absent.
pub fn blocked_tables() -> &'static [(&'static str, &'static str)] {
    &[
        (
            "mob-data.json#states",
            "VanillaMobDataExporter.states() evaluates WalkNodeEvaluator.getPathTypeFromState \
             (and 11 other BlockState predicates) once per entry of Block.BLOCK_STATE_REGISTRY, \
             then writes one row per state. Needs the block-state registry (block order, per-block \
             property cardinalities/value orders and default-state override); route3 has no \
             block-state-id table yet.",
        ),
        (
            "render-definitions.json",
            "VanillaFaceVisibilityOracle writes each block's state-id range and default-state id \
             (Block.getId(state)) plus can_occlude/bars_tag/skipRendering rule. State ids are \
             assigned while the block registry runs code (Blocks.<clinit>, \
             createBlockStateDefinition, registerDefaultState); 746/1,286 blocks override their \
             default state in a constructor. Blocked on the same block-state-id table.",
        ),
        (
            "entity-models.json",
            "VanillaEntityModelExporter bakes LayerDefinitions.createRoots(): ~165 model classes \
             build MeshDefinitions (CubeDeformation, MeshTransformer and ArmorModelSet.map \
             lambdas) and ModelPart.Cube computes polygon vertices/normals/UVs at bake time. This \
             is vanilla's model-baking code, not constants extractable from class files.",
        ),
    ]
}

// ---------------------------------------------------------------------------
// jar access
// ---------------------------------------------------------------------------

/// Minimal read-only view of the client jar.
pub struct Jar {
    archive: zip::ZipArchive<File>,
}

impl Jar {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let archive = zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self { archive })
    }

    pub fn read(&mut self, name: &str) -> Result<Vec<u8>, String> {
        let mut entry = self
            .archive
            .by_name(name)
            .map_err(|e| format!("{name}: {e}"))?;
        let mut data = Vec::with_capacity(entry.size() as usize + 16);
        entry
            .read_to_end(&mut data)
            .map_err(|e| format!("{name}: {e}"))?;
        Ok(data)
    }

    pub fn names(&self) -> Vec<String> {
        self.archive
            .file_names()
            .filter_map(|name| name.ok().map(|name| name.into_owned()))
            .collect()
    }

    fn read_class(&mut self, internal_name: &str) -> Result<Vec<u8>, String> {
        self.read(&format!("{internal_name}.class"))
            .map_err(|e| format!("class {internal_name}: {e}"))
    }

    fn parse_class(&mut self, internal_name: &str) -> Result<ClassFile, String> {
        let data = self.read_class(internal_name)?;
        classfile::parse(&data).map_err(|e| format!("{internal_name}: {e}"))
    }
}

// ---------------------------------------------------------------------------
// class-file helpers
// ---------------------------------------------------------------------------

fn class_constant(class: &ClassFile, index: usize) -> Option<String> {
    match class.constants.get(index) {
        Some(Constant::Class(reference)) => {
            let inner: usize = reference.trim_start_matches('#').parse().ok()?;
            Some(classfile::utf8(&class.constants, inner))
        }
        _ => None,
    }
}

fn code_of(class: &ClassFile, name: &str) -> Result<Vec<u8>, String> {
    let method = class
        .method(name)
        .ok_or_else(|| format!("{}: no {name}", class.name))?;
    let code = method
        .code
        .as_ref()
        .ok_or_else(|| format!("{}.{name}: no code", class.name))?;
    Ok(code.bytes.clone())
}

/// The `Val` a `ldc`/`ldc_w` operand pushes.
fn ldc_value(class: &ClassFile, index: usize) -> Result<Val, String> {
    match class.constants.get(index) {
        Some(Constant::Integer(value)) => Ok(Val::Int(*value)),
        Some(Constant::Float(value)) => Ok(Val::Float(*value)),
        Some(Constant::String(_)) => Ok(Val::Str(
            classfile::string_constant(&class.constants, index)
                .ok_or_else(|| format!("{}: bad String constant #{index}", class.name))?,
        )),
        other => Err(format!(
            "{}: unsupported ldc constant #{index}: {other:?}",
            class.name
        )),
    }
}

fn ldc2_value(class: &ClassFile, index: usize) -> Result<Val, String> {
    if let Some(value) = wide_constant(class, index) {
        return Ok(value);
    }
    // `hzr-bytecode`'s reader assigns the parsed value to `constants[index]`
    // after the `Long`/`Double` arm has already bumped `index`, so a wide
    // constant lands one slot past the index `ldc2_w` names (the named slot
    // stays `Unused`). Read the value it actually wrote; when the reader is
    // fixed this branch is unreachable. See `hzr-bytecode/src/classfile.rs`.
    if let Some(value) = wide_constant(class, index + 1) {
        return Ok(value);
    }
    Err(format!(
        "{}: unsupported ldc2_w constant #{index}: {:?}",
        class.name,
        class.constants.get(index)
    ))
}

fn wide_constant(class: &ClassFile, index: usize) -> Option<Val> {
    match class.constants.get(index) {
        Some(Constant::Long(value)) => Some(Val::Long(*value)),
        Some(Constant::Double(value)) => Some(Val::Double(*value)),
        _ => None,
    }
}

/// Descriptor argument kinds: `D`, `F`, `J`, `I` (all int-like), `A` (reference).
fn arg_kinds(desc: &str) -> Result<Vec<char>, String> {
    let open = desc
        .find('(')
        .ok_or_else(|| format!("bad method descriptor {desc}"))?;
    let close = desc
        .rfind(')')
        .ok_or_else(|| format!("bad method descriptor {desc}"))?;
    let body = &desc[open + 1..close];
    let mut kinds = Vec::new();
    let mut chars = body.bytes().peekable();
    while let Some(byte) = chars.next() {
        match byte {
            b'[' => {
                while let Some(next) = chars.next() {
                    if next == b'L' {
                        for inner in chars.by_ref() {
                            if inner == b';' {
                                break;
                            }
                        }
                        break;
                    }
                    if next != b'[' {
                        break;
                    }
                }
                kinds.push('A');
            }
            b'L' => {
                for inner in chars.by_ref() {
                    if inner == b';' {
                        break;
                    }
                }
                kinds.push('A');
            }
            b'D' => kinds.push('D'),
            b'J' => kinds.push('J'),
            b'F' => kinds.push('F'),
            b'Z' | b'B' | b'C' | b'S' | b'I' => kinds.push('I'),
            other => return Err(format!("bad descriptor arg {other} in {desc}")),
        }
    }
    Ok(kinds)
}

// ---------------------------------------------------------------------------
// bytecode decoder (only the opcodes these initialisers use)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Instr {
    Nop,
    AconstNull,
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Ldc(u16),
    Ldc2(u16),
    Bipush(i32),
    Sipush(i32),
    Pop,
    Pop2,
    Dup,
    ArrayStore(u8),
    Getstatic(u16),
    Putstatic(u16),
    Invokevirtual(u16),
    Invokespecial(u16),
    Invokestatic(u16),
    Invokeinterface(u16),
    Invokedynamic(u16),
    New(u16),
    NewArray,
    ANewArray(u16),
    Arraylength,
    Checkcast(u16),
    I2f,
    F2i,
    F2d,
    D2f,
    D2i,
    I2d,
    I2l,
    L2i,
    Fneg,
    Dneg,
    Areturn,
    Return,
}

fn opcode_name(op: u8) -> &'static str {
    match op {
        0x15 => "iload",
        0x19 => "aload",
        0x36 => "istore",
        0x3a => "astore",
        0x60 => "iadd",
        0x61 => "ladd",
        0x62 => "fadd",
        0x63 => "dadd",
        0x99 => "ifeq",
        0xa7 => "goto",
        0xaa => "tableswitch",
        0xab => "lookupswitch",
        0xb1 => "return",
        0xc4 => "wide",
        _ => "unknown opcode",
    }
}

fn decode(code: &[u8], pc: &mut usize) -> Result<Instr, String> {
    if *pc >= code.len() {
        return Err("instruction pointer past end of code".into());
    }
    let start = *pc;
    let op = code[*pc];
    *pc += 1;
    let read_u1 = |pc: &mut usize| -> Result<u8, String> {
        if *pc >= code.len() {
            return Err(format!("truncated opcode at {start}"));
        }
        let value = code[*pc];
        *pc += 1;
        Ok(value)
    };
    let read_u2 = |pc: &mut usize| -> Result<u16, String> {
        if *pc + 2 > code.len() {
            return Err(format!("truncated opcode at {start}"));
        }
        let value = u16::from_be_bytes([code[*pc], code[*pc + 1]]);
        *pc += 2;
        Ok(value)
    };
    let instr = match op {
        0x00 => Instr::Nop,
        0x01 => Instr::AconstNull,
        0x02..=0x08 => Instr::Int(op as i32 - 3),
        0x09 => Instr::Long(0),
        0x0a => Instr::Long(1),
        0x0b => Instr::Float(0.0),
        0x0c => Instr::Float(1.0),
        0x0d => Instr::Float(2.0),
        0x0e => Instr::Double(0.0),
        0x0f => Instr::Double(1.0),
        0x10 => Instr::Bipush(read_u1(pc)? as i8 as i32),
        0x11 => Instr::Sipush(read_u2(pc)? as i16 as i32),
        0x12 => Instr::Ldc(read_u1(pc)? as u16),
        0x13 => Instr::Ldc(read_u2(pc)?),
        0x14 => Instr::Ldc2(read_u2(pc)?),
        0x4f..=0x56 => Instr::ArrayStore(op),
        0x57 => Instr::Pop,
        0x58 => Instr::Pop2,
        0x59 => Instr::Dup,
        0x85 => Instr::I2l,
        0x86 => Instr::I2f,
        0x87 => Instr::I2d,
        0x88 => Instr::L2i,
        0x8b => Instr::F2i,
        0x8d => Instr::F2d,
        0x8e => Instr::D2i,
        0x90 => Instr::D2f,
        0x94 => Instr::Fneg,
        0x95 => Instr::Dneg,
        0xac..=0xaf => Instr::Return,
        0xb0 => Instr::Areturn,
        0xb1 => Instr::Return,
        0xb2 => Instr::Getstatic(read_u2(pc)?),
        0xb3 => Instr::Putstatic(read_u2(pc)?),
        0xb6 => Instr::Invokevirtual(read_u2(pc)?),
        0xb7 => Instr::Invokespecial(read_u2(pc)?),
        0xb8 => Instr::Invokestatic(read_u2(pc)?),
        0xb9 => {
            let index = read_u2(pc)?;
            read_u1(pc)?;
            read_u1(pc)?;
            Instr::Invokeinterface(index)
        }
        0xba => {
            let index = read_u2(pc)?;
            read_u2(pc)?;
            Instr::Invokedynamic(index)
        }
        0xbb => Instr::New(read_u2(pc)?),
        0xbc => {
            read_u1(pc)?;
            Instr::NewArray
        }
        0xbd => Instr::ANewArray(read_u2(pc)?),
        0xbe => Instr::Arraylength,
        0xc0 => Instr::Checkcast(read_u2(pc)?),
        other => {
            return Err(format!(
                "unsupported opcode {:#04x} ({}) at pc {}",
                other,
                opcode_name(other),
                start
            ))
        }
    };
    Ok(instr)
}

// ---------------------------------------------------------------------------
// value stack
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct FrameState {
    timestamp: f32,
    pre_target: [f32; 3],
    post_target: [f32; 3],
    interpolation: String,
}

#[derive(Clone, Debug)]
struct ChannelState {
    target: String,
    keyframes: Vec<FrameState>,
}

#[derive(Clone, Debug, Default)]
struct BuilderState {
    length: f32,
    looping: bool,
    bones: BTreeMap<String, Vec<ChannelState>>,
}

#[derive(Clone, Debug)]
struct AnimationState {
    length: f32,
    looping: bool,
    bones: BTreeMap<String, Vec<ChannelState>>,
}

#[derive(Clone, Debug)]
enum Val {
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Str(String),
    Vec3([f32; 3]),
    Target(String),
    Interp(String),
    Builder(BuilderState),
    Channel(ChannelState),
    Frame(FrameState),
    Frames(Rc<RefCell<Vec<Option<FrameState>>>>),
    Animation(AnimationState),
    Uninit(String),
    Null,
    Unknown,
}

fn pop(stack: &mut Vec<Val>) -> Result<Val, String> {
    stack
        .pop()
        .ok_or_else(|| "operand stack underflow".to_string())
}

fn push(stack: &mut Vec<Val>, value: Val) {
    stack.push(value);
}

fn duplicate(stack: &mut Vec<Val>) -> Result<(), String> {
    let value = stack
        .last()
        .cloned()
        .ok_or_else(|| "dup on empty stack".to_string())?;
    stack.push(value);
    Ok(())
}

fn pop_args(stack: &mut Vec<Val>, desc: &str) -> Result<Vec<Val>, String> {
    let kinds = arg_kinds(desc)?;
    let mut values = Vec::with_capacity(kinds.len());
    for kind in kinds.iter().rev() {
        let value = pop(stack)?;
        let ok = match kind {
            'D' => matches!(value, Val::Double(_)),
            'F' => matches!(value, Val::Float(_)),
            'J' => matches!(value, Val::Long(_)),
            'I' => matches!(value, Val::Int(_)),
            'A' => !matches!(
                value,
                Val::Int(_) | Val::Long(_) | Val::Float(_) | Val::Double(_)
            ),
            other => return Err(format!("bad argument kind {other}")),
        };
        if !ok {
            return Err(format!(
                "expected {kind} argument for {desc}, got {}",
                value_name(&value)
            ));
        }
        values.push(value);
    }
    values.reverse();
    Ok(values)
}

fn value_name(value: &Val) -> &'static str {
    match value {
        Val::Int(_) => "int",
        Val::Long(_) => "long",
        Val::Float(_) => "float",
        Val::Double(_) => "double",
        Val::Str(_) => "string",
        Val::Vec3(_) => "Vector3f",
        Val::Target(_) => "Target",
        Val::Interp(_) => "Interpolation",
        Val::Builder(_) => "Builder",
        Val::Channel(_) => "AnimationChannel",
        Val::Frame(_) => "Keyframe",
        Val::Frames(_) => "Keyframe[]",
        Val::Animation(_) => "AnimationDefinition",
        Val::Uninit(_) => "uninitialised object",
        Val::Null => "null",
        Val::Unknown => "unknown",
    }
}

fn float_arg(value: &Val) -> Result<f32, String> {
    match value {
        Val::Float(value) => Ok(*value),
        other => Err(format!("expected float, got {}", value_name(other))),
    }
}

fn double_arg(value: &Val) -> Result<f64, String> {
    match value {
        Val::Double(value) => Ok(*value),
        other => Err(format!("expected double, got {}", value_name(other))),
    }
}

fn vec3_arg(value: &Val) -> Result<[f32; 3], String> {
    match value {
        Val::Vec3(value) => Ok(*value),
        other => Err(format!("expected Vector3f, got {}", value_name(other))),
    }
}

fn string_arg(value: &Val) -> Result<String, String> {
    match value {
        Val::Str(value) => Ok(value.clone()),
        other => Err(format!("expected string, got {}", value_name(other))),
    }
}

// ---------------------------------------------------------------------------
// animation definitions
// ---------------------------------------------------------------------------

/// Every `static final AnimationDefinition` field of one definitions class, in
/// class-file (declaration) order, with the value its `<clinit>` builds.
fn animation_definitions(
    jar: &mut Jar,
    internal_name: &str,
) -> Result<Vec<(String, AnimationState)>, String> {
    let class = jar.parse_class(internal_name)?;
    let code = code_of(&class, "<clinit>")?;
    let mut stack: Vec<Val> = Vec::new();
    let mut definitions: Vec<(String, AnimationState)> = Vec::new();
    let mut pc = 0usize;
    loop {
        let instr = decode(&code, &mut pc).map_err(|e| format!("{internal_name}.<clinit>: {e}"))?;
        let context = |what: &str| format!("{internal_name}.<clinit> pc {pc}: {what}");
        match instr {
            Instr::Nop => {}
            Instr::AconstNull => push(&mut stack, Val::Null),
            Instr::Int(value) => push(&mut stack, Val::Int(value)),
            Instr::Long(value) => push(&mut stack, Val::Long(value)),
            Instr::Float(value) => push(&mut stack, Val::Float(value)),
            Instr::Double(value) => push(&mut stack, Val::Double(value)),
            Instr::Ldc(index) => {
                let value = ldc_value(&class, index as usize)?;
                push(&mut stack, value);
            }
            Instr::Ldc2(index) => {
                let value = ldc2_value(&class, index as usize)?;
                push(&mut stack, value);
            }
            Instr::Bipush(value) => push(&mut stack, Val::Int(value)),
            Instr::Sipush(value) => push(&mut stack, Val::Int(value)),
            Instr::Pop => {
                pop(&mut stack)?;
            }
            Instr::Pop2 => {
                pop(&mut stack)?;
                pop(&mut stack)?;
            }
            Instr::Dup => duplicate(&mut stack)?,
            Instr::Getstatic(index) => {
                let (owner, name, _) = classfile::field_ref(&class.constants, index as usize)
                    .ok_or_else(|| context("bad static field reference"))?;
                match (owner.as_str(), name.as_str()) {
                    (ANIMATION_TARGETS, _) => push(&mut stack, Val::Target(name)),
                    (ANIMATION_INTERPOLATIONS, _) => push(&mut stack, Val::Interp(name)),
                    _ => return Err(context(&format!("unexpected static field {owner}.{name}"))),
                }
            }
            Instr::Putstatic(index) => {
                let (owner, name, _) = classfile::field_ref(&class.constants, index as usize)
                    .ok_or_else(|| context("bad static field reference"))?;
                let value = pop(&mut stack)?;
                match value {
                    Val::Animation(animation) if owner == internal_name => {
                        definitions.push((name, animation));
                    }
                    other => {
                        return Err(context(&format!(
                            "unexpected putstatic of {} into {owner}.{name}",
                            value_name(&other)
                        )))
                    }
                }
            }
            Instr::New(index) => {
                let name = class_constant(&class, index as usize)
                    .ok_or_else(|| context("bad class constant for new"))?;
                push(&mut stack, Val::Uninit(name));
            }
            Instr::ANewArray(_) => {
                let length = match pop(&mut stack)? {
                    Val::Int(length) => length,
                    other => {
                        return Err(context(&format!(
                            "anewarray length is {}",
                            value_name(&other)
                        )))
                    }
                };
                if length < 0 {
                    return Err(context("negative array length"));
                }
                let frames: Vec<Option<FrameState>> = vec![None; length as usize];
                push(&mut stack, Val::Frames(Rc::new(RefCell::new(frames))));
            }
            Instr::ArrayStore(op) => {
                if op != 0x53 {
                    return Err(context("unsupported array store"));
                }
                let value = pop(&mut stack)?;
                let index = pop(&mut stack)?;
                let array = pop(&mut stack)?;
                let (Val::Frames(frames), Val::Int(index), Val::Frame(frame)) =
                    (array, index, value)
                else {
                    return Err(context("aastore needs Keyframe[], index, Keyframe"));
                };
                let mut slots = frames.borrow_mut();
                let slot = slots
                    .get_mut(index as usize)
                    .ok_or_else(|| context("aastore index out of bounds"))?;
                *slot = Some(frame);
            }
            Instr::Checkcast(_) => {}
            Instr::Invokestatic(index) => {
                let (owner, name, desc) = classfile::method_ref(&class.constants, index as usize)
                    .ok_or_else(|| context("bad static method reference"))?;
                let args = pop_args(&mut stack, &desc)?;
                let value = match (owner.as_str(), name.as_str()) {
                    (ANIMATION_BUILDER, "withLength") => {
                        let length = float_arg(
                            args.first()
                                .ok_or_else(|| context("withLength: missing length"))?,
                        )?;
                        Val::Builder(BuilderState {
                            length,
                            ..BuilderState::default()
                        })
                    }
                    (KEYFRAME_ANIMATIONS, "posVec") => {
                        // `new Vector3f(x, -y, z)`.
                        let x = float_arg(&args[0])?;
                        let y = float_arg(&args[1])?;
                        let z = float_arg(&args[2])?;
                        Val::Vec3([x, -y, z])
                    }
                    (KEYFRAME_ANIMATIONS, "degreeVec") => {
                        // `new Vector3f(x * 0.017453292f, y * ..., z * ...)`.
                        let x = float_arg(&args[0])?;
                        let y = float_arg(&args[1])?;
                        let z = float_arg(&args[2])?;
                        Val::Vec3([
                            x * DEGREES_TO_RADIANS,
                            y * DEGREES_TO_RADIANS,
                            z * DEGREES_TO_RADIANS,
                        ])
                    }
                    (KEYFRAME_ANIMATIONS, "scaleVec") => {
                        // `(float) (x - 1.0), (float) (y - 1.0), (float) (z - 1.0)`.
                        let x = double_arg(&args[0])?;
                        let y = double_arg(&args[1])?;
                        let z = double_arg(&args[2])?;
                        Val::Vec3([(x - 1.0) as f32, (y - 1.0) as f32, (z - 1.0) as f32])
                    }
                    _ => {
                        return Err(context(&format!(
                            "unexpected static call {owner}.{name}{desc}"
                        )))
                    }
                };
                push(&mut stack, value);
            }
            Instr::Invokespecial(index) => {
                let (owner, name, desc) = classfile::method_ref(&class.constants, index as usize)
                    .ok_or_else(|| context("bad constructor reference"))?;
                if name != "<init>" {
                    return Err(context(&format!("unexpected invokespecial {owner}.{name}")));
                }
                let args = pop_args(&mut stack, &desc)?;
                let receiver = pop(&mut stack)?;
                let Val::Uninit(receiver_owner) = receiver else {
                    return Err(context("constructor receiver is not uninitialised object"));
                };
                let value = match owner.as_str() {
                    KEYFRAME => {
                        let (timestamp, pre, post, interpolation) = match args.len() {
                            3 => (
                                float_arg(&args[0])?,
                                vec3_arg(&args[1])?,
                                vec3_arg(&args[1])?,
                                args[2].clone(),
                            ),
                            4 => (
                                float_arg(&args[0])?,
                                vec3_arg(&args[1])?,
                                vec3_arg(&args[2])?,
                                args[3].clone(),
                            ),
                            count => {
                                return Err(context(&format!(
                                    "Keyframe constructor takes 3 or 4 args, got {count}"
                                )))
                            }
                        };
                        let Val::Interp(interpolation) = interpolation else {
                            return Err(context("Keyframe interpolation is not an Interpolation"));
                        };
                        Val::Frame(FrameState {
                            timestamp,
                            pre_target: pre,
                            post_target: post,
                            interpolation,
                        })
                    }
                    ANIMATION_CHANNEL => {
                        let Val::Target(target) = args.first().cloned().unwrap_or(Val::Unknown)
                        else {
                            return Err(context("AnimationChannel target is not a Target"));
                        };
                        let Val::Frames(frames) = args
                            .get(1)
                            .cloned()
                            .ok_or_else(|| context("AnimationChannel: missing keyframes"))?
                        else {
                            return Err(context("AnimationChannel keyframes are not Keyframe[]"));
                        };
                        let keyframes = frames
                            .borrow()
                            .iter()
                            .map(|frame| {
                                frame.clone().ok_or_else(|| {
                                    context("AnimationChannel has an unset keyframe slot")
                                })
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        Val::Channel(ChannelState { target, keyframes })
                    }
                    VECTOR3F => {
                        // Some initialisers build a Vector3f directly.
                        let x = float_arg(&args[0])?;
                        let y = float_arg(&args[1])?;
                        let z = float_arg(&args[2])?;
                        Val::Vec3([x, y, z])
                    }
                    other => {
                        return Err(context(&format!(
                        "unexpected constructor {other}.<init>{desc} (receiver {receiver_owner})"
                    )))
                    }
                };
                // `new X; dup; <args>; invokespecial X.<init>` leaves the
                // second (unconsumed) reference on the stack: the constructor
                // turns that duplicate into the initialised object.
                match stack.last_mut() {
                    Some(slot @ Val::Uninit(_)) => *slot = value,
                    _ => return Err(context(
                        "constructor did not leave its duplicated object reference on the stack",
                    )),
                }
            }
            Instr::Invokevirtual(index) => {
                let (owner, name, desc) = classfile::method_ref(&class.constants, index as usize)
                    .ok_or_else(|| context("bad virtual method reference"))?;
                let args = pop_args(&mut stack, &desc)?;
                let receiver = pop(&mut stack)?;
                let mut builder = match receiver {
                    Val::Builder(builder) => builder,
                    other => {
                        return Err(context(&format!(
                            "unexpected virtual call {owner}.{name} on {}",
                            value_name(&other)
                        )))
                    }
                };
                match name.as_str() {
                    "looping" => builder.looping = true,
                    "addAnimation" => {
                        let bone = string_arg(
                            args.first()
                                .ok_or_else(|| context("addAnimation: missing bone"))?,
                        )?;
                        let channel = match args.get(1) {
                            Some(Val::Channel(channel)) => channel.clone(),
                            other => {
                                return Err(context(&format!(
                                    "addAnimation: channel is {}",
                                    other.map(value_name).unwrap_or("missing")
                                )))
                            }
                        };
                        builder.bones.entry(bone).or_default().push(channel);
                    }
                    "build" => {
                        let animation = AnimationState {
                            length: builder.length,
                            looping: builder.looping,
                            bones: builder.bones,
                        };
                        push(&mut stack, Val::Animation(animation));
                        continue;
                    }
                    other => {
                        return Err(context(&format!("unexpected Builder method {other}{desc}")))
                    }
                }
                push(&mut stack, Val::Builder(builder));
            }
            Instr::Return => break,
            other => return Err(context(&format!("unsupported instruction {other:?}"))),
        }
    }
    Ok(definitions)
}

/// Every keyframe animation of the client's entity models, keyed
/// `SimpleClassName.FIELD_NAME` like `VanillaMobDataExporter.animations()`.
fn animations(jar: &mut Jar) -> Result<Value, String> {
    let prefix = ANIMATION_DEFINITIONS;
    let mut classes: Vec<String> = jar
        .names()
        .into_iter()
        .filter(|name| {
            name.starts_with(prefix)
                && name.ends_with(".class")
                && !name.contains('$')
                && !name.ends_with("package-info.class")
        })
        .collect();
    // The Java exporter collects the internal class names into a TreeSet.
    classes.sort();

    let mut out = Map::new();
    for class_name in classes {
        let internal = class_name
            .strip_suffix(".class")
            .ok_or_else(|| format!("{class_name}: not a class file"))?;
        let simple = internal.rsplit('/').next().unwrap_or(internal);
        for (field, animation) in animation_definitions(jar, internal)? {
            out.insert(format!("{simple}.{field}"), animation_value(&animation));
        }
    }
    Ok(Value::Object(out))
}

fn animation_value(animation: &AnimationState) -> Value {
    let mut object = Map::new();
    object.insert("length".into(), f32_value(animation.length));
    object.insert("looping".into(), Value::Bool(animation.looping));
    let mut bones = Map::new();
    for (bone, channels) in &animation.bones {
        let channels = channels.iter().map(channel_value).collect();
        bones.insert(bone.clone(), Value::Array(channels));
    }
    object.insert("bones".into(), Value::Object(bones));
    Value::Object(object)
}

fn channel_value(channel: &ChannelState) -> Value {
    let mut object = Map::new();
    object.insert(
        "target".into(),
        Value::String(target_name(&channel.target).to_string()),
    );
    let frames = channel
        .keyframes
        .iter()
        .map(|frame| {
            Value::Array(vec![
                f32_value(frame.timestamp),
                vec3_value(frame.pre_target),
                vec3_value(frame.post_target),
                Value::String(interpolation_name(&frame.interpolation).to_string()),
            ])
        })
        .collect();
    object.insert("keyframes".into(), Value::Array(frames));
    Value::Object(object)
}

/// `AnimationChannel.Targets` -> the name `VanillaMobDataExporter` writes.
fn target_name(field: &str) -> &'static str {
    match field {
        "POSITION" => "position",
        "ROTATION" => "rotation",
        _ => "scale",
    }
}

/// `AnimationChannel.Interpolations` -> the name `VanillaMobDataExporter` writes.
fn interpolation_name(field: &str) -> &'static str {
    if field == "CATMULLROM" {
        "catmullrom"
    } else {
        "linear"
    }
}

fn vec3_value(value: [f32; 3]) -> Value {
    Value::Array(
        value
            .iter()
            .map(|component| f32_value(*component))
            .collect(),
    )
}

/// A JSON number whose text is Java's `Float.toString`, i.e. the shortest
/// decimal that round-trips to the same `f32`.
///
/// `serde_json` only stores `f64` numbers, so the shortest `f32` text is parsed
/// back as `f64`: the table then holds exactly the number Java wrote (a Java
/// float is what the column means), and the parity test compares it at `f32`
/// precision.
fn f32_value(value: f32) -> Value {
    if !value.is_finite() {
        // The Java exporters never write a non-finite animation number; fail
        // loudly rather than silently writing a wrong value.
        return Value::String(format!("non-finite:{value}"));
    }
    let text = format!("{value}");
    let number: f64 = text
        .parse()
        .unwrap_or_else(|e| panic!("f32 Display {text:?} is not a number: {e}"));
    Value::Number(Number::from_f64(number).expect("parsed f64 is finite"))
}

// ---------------------------------------------------------------------------
// entity types and spawn placements
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct EntityRegistry {
    /// Registry order, i.e. `BuiltInRegistries.ENTITY_TYPE` iteration order.
    order: Vec<String>,
    /// `EntityTypes.<FIELD>` -> registry id (without the `minecraft:` prefix).
    field_to_id: HashMap<String, String>,
}

/// `EntityTypeIds.<clinit>`: `EntityTypes` field name -> id string.
fn entity_type_ids(jar: &mut Jar) -> Result<HashMap<String, String>, String> {
    let class = jar.parse_class(ENTITY_TYPE_IDS)?;
    let code = code_of(&class, "<clinit>")?;
    let mut ids = HashMap::new();
    let mut pending: Option<String> = None;
    let mut pc = 0usize;
    loop {
        let instr =
            decode(&code, &mut pc).map_err(|e| format!("{ENTITY_TYPE_IDS}.<clinit>: {e}"))?;
        match instr {
            Instr::Nop => {}
            Instr::Ldc(index) => {
                if let Val::Str(text) = ldc_value(&class, index as usize)? {
                    pending = Some(text);
                }
            }
            Instr::Putstatic(index) => {
                let (owner, name, _) = classfile::field_ref(&class.constants, index as usize)
                    .ok_or_else(|| format!("{ENTITY_TYPE_IDS}.<clinit>: bad field reference"))?;
                if owner == ENTITY_TYPE_IDS {
                    if let Some(id) = pending.take() {
                        ids.insert(name, id);
                    }
                }
            }
            Instr::Invokestatic(_) => {}
            Instr::Return => break,
            _ => {}
        }
    }
    Ok(ids)
}

/// `EntityTypes.<clinit>`: the registration order and the field -> id map.
fn entity_registry(jar: &mut Jar) -> Result<EntityRegistry, String> {
    let ids = entity_type_ids(jar)?;
    let class = jar.parse_class(ENTITY_TYPES)?;
    let code = code_of(&class, "<clinit>")?;
    let mut registry = EntityRegistry::default();
    let mut pending_id: Option<String> = None;
    let mut registered: Option<String> = None;
    let mut pc = 0usize;
    loop {
        let instr = decode(&code, &mut pc).map_err(|e| format!("{ENTITY_TYPES}.<clinit>: {e}"))?;
        match instr {
            Instr::Getstatic(index) => {
                let (owner, name, _) = classfile::field_ref(&class.constants, index as usize)
                    .ok_or_else(|| format!("{ENTITY_TYPES}.<clinit>: bad static field"))?;
                if owner == ENTITY_TYPE_IDS {
                    if let Some(id) = ids.get(&name) {
                        pending_id = Some(id.clone());
                    }
                }
            }
            Instr::Invokestatic(index) => {
                let (_, name, desc) = classfile::method_ref(&class.constants, index as usize)
                    .ok_or_else(|| format!("{ENTITY_TYPES}.<clinit>: bad method reference"))?;
                if name == "register" && desc.contains(ENTITY_TYPE_BUILDER) {
                    let id = pending_id.take().ok_or_else(|| {
                        format!("{ENTITY_TYPES}.<clinit>: register without an EntityTypeIds id")
                    })?;
                    registry.order.push(id.clone());
                    registered = Some(id);
                }
            }
            Instr::Putstatic(index) => {
                let (owner, name, _) = classfile::field_ref(&class.constants, index as usize)
                    .ok_or_else(|| format!("{ENTITY_TYPES}.<clinit>: bad static field"))?;
                if let Some(id) = registered.take() {
                    if owner == ENTITY_TYPES {
                        registry.field_to_id.insert(name, id);
                    }
                }
            }
            Instr::Return => break,
            _ => {}
        }
    }
    Ok(registry)
}

/// `SpawnPlacements.<clinit>`: `EntityTypes` field -> (placement, heightmap).
fn spawn_placements(jar: &mut Jar) -> Result<HashMap<String, (String, String)>, String> {
    let class = jar.parse_class(SPAWN_PLACEMENTS)?;
    let code = code_of(&class, "<clinit>")?;
    let mut placements = HashMap::new();
    let mut entity_field: Option<String> = None;
    let mut placement: Option<String> = None;
    let mut heightmap: Option<String> = None;
    let mut pc = 0usize;
    loop {
        let instr =
            decode(&code, &mut pc).map_err(|e| format!("{SPAWN_PLACEMENTS}.<clinit>: {e}"))?;
        match instr {
            Instr::Getstatic(index) => {
                let (owner, name, _) = classfile::field_ref(&class.constants, index as usize)
                    .ok_or_else(|| format!("{SPAWN_PLACEMENTS}.<clinit>: bad static field"))?;
                match owner.as_str() {
                    ENTITY_TYPES => entity_field = Some(name),
                    SPAWN_PLACEMENT_TYPES => placement = Some(name),
                    HEIGHTMAP_TYPES => heightmap = Some(name),
                    _ => {}
                }
            }
            Instr::Invokestatic(index) => {
                let (_, name, desc) = classfile::method_ref(&class.constants, index as usize)
                    .ok_or_else(|| format!("{SPAWN_PLACEMENTS}.<clinit>: bad method reference"))?;
                if name == "register" && desc.contains("SpawnPlacementType") {
                    let field = entity_field.take().ok_or_else(|| {
                        format!("{SPAWN_PLACEMENTS}.<clinit>: register without a type")
                    })?;
                    let placement = placement.take().ok_or_else(|| {
                        format!("{SPAWN_PLACEMENTS}.<clinit>: register without a placement")
                    })?;
                    let heightmap = heightmap.take().ok_or_else(|| {
                        format!("{SPAWN_PLACEMENTS}.<clinit>: register without a heightmap")
                    })?;
                    placements.insert(field, (placement, heightmap));
                }
            }
            Instr::Return => break,
            _ => {}
        }
    }
    Ok(placements)
}

/// `Heightmap$Types.<clinit>`: enum constant field -> `getSerializedName()`.
fn heightmap_serialized_names(jar: &mut Jar) -> Result<HashMap<String, String>, String> {
    let class = jar.parse_class(HEIGHTMAP_TYPES)?;
    let code = code_of(&class, "<clinit>")?;
    let mut names = HashMap::new();
    let mut pending_key: Option<String> = None;
    let mut constant_key: Option<String> = None;
    let mut pc = 0usize;
    loop {
        let instr =
            decode(&code, &mut pc).map_err(|e| format!("{HEIGHTMAP_TYPES}.<clinit>: {e}"))?;
        match instr {
            Instr::Ldc(index) => {
                if let Val::Str(text) = ldc_value(&class, index as usize)? {
                    pending_key = Some(text);
                }
            }
            Instr::Invokespecial(index) => {
                let (owner, name, _) = classfile::method_ref(&class.constants, index as usize)
                    .ok_or_else(|| format!("{HEIGHTMAP_TYPES}.<clinit>: bad method reference"))?;
                if owner == HEIGHTMAP_TYPES && name == "<init>" {
                    constant_key = pending_key.take();
                }
            }
            Instr::Putstatic(index) => {
                let (owner, name, _) = classfile::field_ref(&class.constants, index as usize)
                    .ok_or_else(|| format!("{HEIGHTMAP_TYPES}.<clinit>: bad static field"))?;
                if owner == HEIGHTMAP_TYPES {
                    if let Some(key) = constant_key.take() {
                        names.insert(name, key);
                    }
                }
            }
            Instr::Return => break,
            _ => {}
        }
    }
    Ok(names)
}

fn placement_name(field: &str) -> &'static str {
    match field {
        "ON_GROUND" => "on_ground",
        "IN_WATER" => "in_water",
        "IN_LAVA" => "in_lava",
        _ => "no_restrictions",
    }
}

fn placements_value(jar: &mut Jar) -> Result<Value, String> {
    let registry = entity_registry(jar)?;
    let registered = spawn_placements(jar)?;
    let heightmap_names = heightmap_serialized_names(jar)?;
    let by_id: HashMap<&str, &(String, String)> = registry
        .field_to_id
        .iter()
        .filter_map(|(field, id)| registered.get(field).map(|data| (id.as_str(), data)))
        .collect();

    let mut out = Map::new();
    for id in &registry.order {
        let (placement, heightmap) = match by_id.get(id.as_str()) {
            Some((placement, heightmap)) => (
                placement_name(placement),
                heightmap_names
                    .get(heightmap)
                    .cloned()
                    .unwrap_or_else(|| heightmap.clone()),
            ),
            None => ("no_restrictions", "MOTION_BLOCKING_NO_LEAVES".to_string()),
        };
        let mut row = Map::new();
        row.insert("placement".into(), Value::String(placement.to_string()));
        row.insert("heightmap".into(), Value::String(heightmap));
        out.insert(format!("minecraft:{id}"), Value::Object(row));
    }
    Ok(Value::Object(out))
}

// ---------------------------------------------------------------------------
// public API
// ---------------------------------------------------------------------------

/// The sections of `mob-data.json` this module rebuilds.
pub struct MobData {
    /// `mob-data.json#states` is blocked; see [`blocked_tables`].
    pub states: Option<Value>,
    pub placements: Value,
    pub animations: Value,
}

impl MobData {
    /// The table as JSON: implemented sections only (no `states`).
    pub fn to_value(&self) -> Value {
        let mut root = Map::new();
        if let Some(states) = &self.states {
            root.insert("states".into(), states.clone());
        }
        root.insert("placements".into(), self.placements.clone());
        root.insert("animations".into(), self.animations.clone());
        Value::Object(root)
    }
}

/// Rebuild the implemented sections of `mob-data.json` from the client jar.
pub fn export_mob_data(jar_path: impl AsRef<Path>) -> Result<MobData, String> {
    let mut jar = Jar::open(jar_path)?;
    Ok(MobData {
        states: None,
        placements: placements_value(&mut jar)?,
        animations: animations(&mut jar)?,
    })
}

pub fn write_mob_data(jar_path: impl AsRef<Path>, out: impl AsRef<Path>) -> Result<(), String> {
    let value = export_mob_data(jar_path)?.to_value();
    let out = out.as_ref();
    let text = serde_json::to_string(&value).map_err(|e| e.to_string())?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(out, text).map_err(|e| format!("{}: {e}", out.display()))
}

/// The entity-type registry order (`BuiltInRegistries.ENTITY_TYPE` iteration
/// order), as ids without the `minecraft:` prefix.
///
/// `serde_json`'s map keeps keys sorted, so the generated placements object
/// cannot show the order Java wrote; this exposes it for parity tests and for
/// callers that need registry order.
pub fn entity_type_order(jar_path: impl AsRef<Path>) -> Result<Vec<String>, String> {
    let mut jar = Jar::open(jar_path)?;
    Ok(entity_registry(&mut jar)?.order)
}
