//! Last constructor-value closure for `block-behaviours.json#blocks[*].fields`.
//!
//! `behaviour_fields_ctor` derives 1,255 of the 1,286 registry blocks and reports
//! the remaining 31. Every one of those is blocked by a value that is not a
//! simple constructor literal:
//!
//! * the 15 buttons carry a `BlockSetType` record (built in `BlockSetType.<clinit>`
//!   from a `SoundType` record and eight `SoundEvents` ids) plus the
//!   `ticksToStayPressed` int from `new ButtonBlock(BlockSetType.X, ticks, props)`;
//! * the four amethyst clusters carry the `height`/`width` float arguments of
//!   `new AmethystClusterBlock(height, width, props)` and a
//!   `Map<Direction, VoxelShape>` filled by `Shapes.rotateAll`;
//! * the four growing-plant heads pass the growth probability as the final double
//!   of `GrowingPlantHeadBlock.<init>`;
//! * `bush`/`red_shrub` take `BushBlock`'s default `shapeHeight`;
//! * `fire`'s odds/damage are `FireBlock`'s own startup tables (reused from
//!   `behaviour_header::fire`);
//! * the three multiface blocks derive the three booleans from their face set;
//! * `hopper` builds its interaction map from `Shapes.rotateHorizontal` + `DOWN`;
//! * `redstone_wire`'s `crossState` is a `BlockState` whose state id is resolved
//!   through the jar-derived state table, and its `evaluator` is the concrete
//!   class its constructor instantiates.
//!
//! Nothing here reads `ship.hzr3`; every value is derived from the player's jar.

#![allow(
    dead_code,
    clippy::duplicate_mod,
    clippy::empty_line_after_doc_comments,
    clippy::too_many_arguments
)]

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

use serde_json::{Map, Number, Value};

#[path = "behaviour_fields_ctor.rs"]
#[allow(unused, clippy::all)]
pub mod behaviour_fields_ctor;

#[path = "behaviour_fields_full.rs"]
#[allow(unused, clippy::all)]
mod behaviour_fields_full;

#[path = "behaviour_header.rs"]
#[allow(unused, clippy::all)]
mod behaviour_header;

#[path = "states.rs"]
#[allow(unused, clippy::all)]
mod states;

#[path = "block.rs"]
#[allow(unused, clippy::all)]
mod block;

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

use classfile::{ClassFile, Constant};

const BLOCKS: &str = "net/minecraft/world/level/block/Blocks";
const BLOCK: &str = "net/minecraft/world/level/block/Block";
const BUTTON: &str = "net/minecraft/world/level/block/ButtonBlock";
const AMETHYST: &str = "net/minecraft/world/level/block/AmethystClusterBlock";
const BUSH: &str = "net/minecraft/world/level/block/BushBlock";
const MULTIFACE: &str = "net/minecraft/world/level/block/MultifaceBlock";
const REDSTONE_WIRE: &str = "net/minecraft/world/level/block/RedstoneWireBlock";
const BLOCK_SET_TYPE: &str = "net/minecraft/world/level/block/state/properties/BlockSetType";
const SOUND_TYPE: &str = "net/minecraft/world/level/block/SoundType";
const SOUND_EVENTS: &str = "net/minecraft/sounds/SoundEvents";

const REGISTER_HELPERS: [&str; 5] = [
    "register",
    "registerStair",
    "registerSlab",
    "registerWall",
    "registerLegacyStair",
];

const HORIZONTAL: [&str; 4] = ["NORTH", "SOUTH", "EAST", "WEST"];
const DIRECTIONS: [&str; 6] = ["DOWN", "UP", "NORTH", "SOUTH", "WEST", "EAST"];

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub blocks: usize,
    /// Blocks `behaviour_fields_ctor` already closed.
    pub via_ctor: usize,
    /// Of the ctor's remaining blocks, the ones this module finished.
    pub finished: usize,
    pub remaining: Vec<String>,
    pub unresolved_fields: BTreeMap<String, Vec<String>>,
}

/// All block `fields` maps, exact for the whole registry after normalization.
pub fn normalized_fields(jar_path: &Path) -> Result<(BTreeMap<String, Value>, Coverage), String> {
    let (mut out, ctor_coverage) = behaviour_fields_ctor::normalized_fields(jar_path)?;
    let (_, all, _) = behaviour_fields_full::block_fields_report(jar_path)?;
    let registry = block::registry(jar_path)?;
    let jar = Jar::open(jar_path)?;
    let registrations = registration_args(&jar, &registry)?;
    let block_set_types = block_set_type_records(&jar)?;
    let sound_types = sound_type_records(&jar)?;
    let sound_ids = sound_event_ids(&jar);
    let fire = behaviour_header::fire(jar_path)?;
    let mut state_table: Option<states::StateTable> = None;

    let mut coverage = Coverage {
        blocks: registry.len(),
        via_ctor: out.len(),
        ..Default::default()
    };

    for id in &ctor_coverage.remaining {
        let Some(row) = registry.iter().find(|row| &row.id == id) else {
            coverage.remaining.push(id.clone());
            continue;
        };
        let Some(internal) = jar.resolve(&row.class) else {
            coverage.remaining.push(id.clone());
            continue;
        };
        let existing = all
            .get(id)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let missing: Vec<(String, String)> = declared_fields(&jar, &internal)
            .into_iter()
            .filter(|(name, _)| !existing.contains_key(name))
            .collect();
        let args = registrations.get(id).cloned().unwrap_or_default();
        let mut fields = existing;
        let mut unresolved = Vec::new();
        for (name, descriptor) in &missing {
            let value = field_value(
                &jar,
                id,
                &internal,
                name,
                descriptor,
                &args,
                &block_set_types,
                &sound_types,
                &sound_ids,
                &fire,
                &mut state_table,
                jar_path,
            );
            match value {
                Some(value) => {
                    fields.insert(name.clone(), value);
                }
                None => unresolved.push(name.clone()),
            }
        }
        if unresolved.is_empty() {
            out.insert(id.clone(), Value::Object(fields));
            coverage.finished += 1;
        } else {
            coverage.remaining.push(id.clone());
            coverage.unresolved_fields.insert(id.clone(), unresolved);
        }
    }
    Ok((out, coverage))
}

/// One missing field, derived from the registration call or the class' own
/// constructor.
#[allow(clippy::too_many_arguments)]
fn field_value(
    jar: &Jar,
    id: &str,
    internal: &str,
    name: &str,
    descriptor: &str,
    registration: &Registration,
    block_set_types: &BTreeMap<String, Value>,
    sound_types: &BTreeMap<String, SoundTypeRecord>,
    sound_ids: &BTreeMap<String, String>,
    fire: &Value,
    state_table: &mut Option<states::StateTable>,
    jar_path: &Path,
) -> Option<Value> {
    let chain = class_chain(jar, internal);
    match name {
        "ticksToStayPressed" if chain.iter().any(|class| class == "ButtonBlock") => {
            registration.ints().next().map(|ticks| Value::from(ticks))
        }
        "type" if chain.iter().any(|class| class == "ButtonBlock") => {
            let field = registration.fields().next()?;
            block_set_types.get(field).cloned()
        }
        "height" if class_is(jar, internal, AMETHYST) => registration
            .floats()
            .next()
            .map(|value| Value::from(value as f64)),
        "width" if class_is(jar, internal, AMETHYST) => registration
            .floats()
            .nth(1)
            .map(|value| Value::from(value as f64)),
        "shapes" if class_is(jar, internal, AMETHYST) => Some(direction_shape_map(&DIRECTIONS)),
        "growPerTickProbability" => growth_probability(jar, internal).map(Value::from),
        "shapeHeight" => bush_shape_height(jar, internal).map(Value::from),
        "igniteOdds" | "burnOdds" | "fireDamage" => fire.get(name).cloned(),
        "canRotate" | "canMirrorX" | "canMirrorZ"
            if chain.iter().any(|class| class == "MultifaceBlock") =>
        {
            let support = multiface_support(jar, internal)?;
            Some(Value::Bool(match name {
                "canRotate" => HORIZONTAL.iter().all(|direction| support(direction)),
                "canMirrorX" => {
                    ["EAST", "WEST"]
                        .iter()
                        .filter(|direction| support(direction))
                        .count()
                        % 2
                        == 0
                }
                _ => {
                    ["NORTH", "SOUTH"]
                        .iter()
                        .filter(|direction| support(direction))
                        .count()
                        % 2
                        == 0
                }
            }))
        }
        "interactionShapes" if chain.iter().any(|class| class == "HopperBlock") => {
            hopper_interaction_shapes(jar, internal)
        }
        "crossState" if class_is(jar, internal, REDSTONE_WIRE) => {
            if state_table.is_none() {
                *state_table = Some(states::state_table(jar_path).ok()?);
            }
            let table = state_table.as_ref()?;
            redstone_cross_state(jar, internal, table).map(|id| {
                let mut object = Map::new();
                object.insert("state".to_string(), Value::Number(Number::from(id)));
                Value::Object(object)
            })
        }
        "evaluator" if class_is(jar, internal, REDSTONE_WIRE) => redstone_evaluator(jar, internal),
        _ => {
            if let Some(value) = ctor_literal_value(jar, internal, name, descriptor) {
                return Some(value);
            }
            // Last resort: the exporter's identity token for concrete classes.
            let _ = (descriptor, sound_types, sound_ids);
            identity_token(jar, descriptor)
                .map(Value::String)
                .map(|value| {
                    let _ = id;
                    value
                })
        }
    }
}

/// `this.field = <constant>` in the constructor chain (e.g. `shouldSignal`).
fn ctor_literal_value(jar: &Jar, internal: &str, name: &str, descriptor: &str) -> Option<Value> {
    let mut current = Some(internal.to_string());
    for _ in 0..64 {
        let Some(class_name) = current else { break };
        if class_name == BLOCK {
            break;
        }
        let class = jar.class(&class_name)?;
        for method in &class.methods {
            if method.name != "<init>" {
                continue;
            }
            let Some(code) = method.code.as_ref() else {
                continue;
            };
            let instructions = decode(&code.bytes);
            let mut last: Option<Value> = None;
            let mut pending_this = false;
            for instruction in &instructions {
                match instruction.opcode {
                    0x2a => pending_this = true,
                    0x59 | 0x57 => {}
                    0x02..=0x08 | 0x0b..=0x0d | 0x0e..=0x0f | 0x10 | 0x11 | 0x12 | 0x13 | 0x14 => {
                        last = push_arg(&class, instruction).map(|arg| match arg {
                            Arg::Int(value) => Value::from(value),
                            Arg::Float(value) => Value::from(value as f64),
                            Arg::Double(value) => Value::from(value),
                            Arg::Str(value) => Value::String(value),
                            Arg::Field(_, field) => Value::String(field),
                        });
                    }
                    0xb5 => {
                        if pending_this {
                            if let Some((owner, field, _)) = field_ref(&class, instruction.index) {
                                if owner == class_name && field == name {
                                    let value = last.take()?;
                                    if descriptor == "Z" {
                                        return Some(Value::Bool(
                                            value
                                                .as_i64()
                                                .map(|number| number != 0)
                                                .unwrap_or(false),
                                        ));
                                    }
                                    return Some(value);
                                }
                            }
                            pending_this = false;
                        }
                    }
                    _ => pending_this = false,
                }
            }
        }
        current = if class.super_name.is_empty() {
            None
        } else {
            Some(class.super_name.clone())
        };
    }
    None
}

fn identity_token(jar: &Jar, descriptor: &str) -> Option<String> {
    let class = descriptor
        .trim_start_matches('[')
        .trim_start_matches('L')
        .trim_end_matches(';');
    if matches!(class, "D" | "F" | "I" | "J" | "S" | "B" | "C" | "Z")
        || class == "java/lang/String"
        || class.ends_with("BlockSetType")
        || class.ends_with("BlockState")
        || class.ends_with("RedstoneWireEvaluator")
    {
        return None;
    }
    if class.ends_with("VoxelShape") {
        return Some("<VoxelShape>".to_string());
    }
    if class.starts_with("java/util/function/") || class.contains("Function") {
        return Some("<Lambda>".to_string());
    }
    let access = jar.access(class)?;
    if access & 0x0200 != 0 || access & 0x0400 != 0 {
        return None;
    }
    let simple = class.rsplit('/').next().unwrap_or(class);
    Some(format!("<{simple}>"))
}

// ---------------------------------------------------------------------------
// Registry order, classes and fields
// ---------------------------------------------------------------------------

fn class_is(jar: &Jar, internal: &str, wanted: &str) -> bool {
    let mut current = internal.to_string();
    for _ in 0..64 {
        if current == wanted {
            return true;
        }
        let Some(class) = jar.class(&current) else {
            return false;
        };
        if class.super_name.is_empty() || class.super_name == "java/lang/Object" {
            return false;
        }
        current = class.super_name.clone();
    }
    false
}

fn class_chain(jar: &Jar, internal: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = internal.to_string();
    for _ in 0..64 {
        if current == BLOCK || current.is_empty() {
            break;
        }
        let Some(class) = jar.class(&current) else {
            break;
        };
        out.push(simple(&class.name).to_string());
        if class.super_name.is_empty() {
            break;
        }
        current = class.super_name.clone();
    }
    out
}

fn declared_fields(jar: &Jar, internal: &str) -> Vec<(String, String)> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut current = Some(internal.to_string());
    for _ in 0..64 {
        let Some(name) = current else { break };
        if name == BLOCK {
            break;
        }
        let Some(parsed) = jar.class(&name) else {
            break;
        };
        for field in &parsed.fields {
            if field.access & 0x0008 != 0 || !seen.insert(field.name.clone()) {
                continue;
            }
            out.push((field.name.clone(), field.descriptor.clone()));
        }
        current = if parsed.super_name.is_empty() {
            None
        } else {
            Some(parsed.super_name.clone())
        };
    }
    out
}

fn simple(internal: &str) -> &str {
    internal.rsplit('/').next().unwrap_or(internal)
}

// ---------------------------------------------------------------------------
// Registration call sites
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
struct Registration {
    class: String,
    args: Vec<Arg>,
}

impl Registration {
    fn ints(&self) -> impl Iterator<Item = i32> + '_ {
        self.args.iter().filter_map(|arg| match arg {
            Arg::Int(value) => Some(*value),
            _ => None,
        })
    }

    fn floats(&self) -> impl Iterator<Item = f32> + '_ {
        self.args.iter().filter_map(|arg| match arg {
            Arg::Float(value) => Some(*value),
            _ => None,
        })
    }

    fn fields(&self) -> impl Iterator<Item = &str> + '_ {
        self.args.iter().filter_map(|arg| match arg {
            Arg::Field(_, field) => Some(field.as_str()),
            _ => None,
        })
    }
}

#[derive(Debug, Clone)]
enum Arg {
    Int(i32),
    Float(f32),
    Double(f64),
    Str(String),
    Field(String, String),
}

/// `block id -> the constructor its registration lambda invokes`.
fn registration_args(
    jar: &Jar,
    registry: &[block::Block],
) -> Result<BTreeMap<String, Registration>, String> {
    let parsed = jar
        .class(BLOCKS)
        .ok_or_else(|| "Blocks is not in the jar".to_string())?;
    let Some(code) = parsed
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
    else {
        return Err("Blocks has no initialiser".to_string());
    };
    let instructions = decode(&code.bytes);
    let by_key: BTreeMap<String, String> = registry
        .iter()
        .map(|row| {
            (
                row.id
                    .strip_prefix("minecraft:")
                    .unwrap_or(&row.id)
                    .to_uppercase(),
                row.id.clone(),
            )
        })
        .collect();
    let mut out = BTreeMap::new();
    let mut pending_id: Option<String> = None;
    let mut indys: Vec<usize> = Vec::new();
    for instruction in &instructions {
        match instruction.opcode {
            0xb2 => {
                let Some((owner, field, _)) = field_ref(&parsed, instruction.index) else {
                    continue;
                };
                if owner.ends_with("references/BlockItemIds")
                    || owner.ends_with("references/BlockIds")
                {
                    if let Some(id) = by_key.get(&field.to_uppercase()) {
                        pending_id = Some(id.clone());
                        indys.clear();
                    }
                }
            }
            0xba => indys.push(instruction.index),
            0xb8 => {
                let Some((owner, name, _)) = method_ref(&parsed, instruction.index) else {
                    continue;
                };
                if owner != BLOCKS || !REGISTER_HELPERS.contains(&name.as_str()) {
                    continue;
                }
                if let (Some(id), Some(indy)) = (pending_id.take(), indys.first().copied()) {
                    if let Some(registration) = resolve_registration(jar, &parsed, indy) {
                        out.insert(id, registration);
                    }
                }
                indys.clear();
            }
            _ => {}
        }
    }
    Ok(out)
}

fn resolve_registration(jar: &Jar, blocks: &ClassFile, indy: usize) -> Option<Registration> {
    let (owner, name, descriptor) = indy_target(blocks, indy)?;
    if name == "<init>" {
        return Some(Registration {
            class: owner,
            args: Vec::new(),
        });
    }
    let target = jar.class(&owner)?;
    let method = target
        .methods
        .iter()
        .find(|method| method.name == name && method.descriptor == descriptor)?;
    let code = method.code.as_ref()?;
    let (class, args) = new_call_args(&target, &code.bytes)?;
    Some(Registration { class, args })
}

/// Constants pushed into the first `new X(...)` of a method.
fn new_call_args(class: &ClassFile, bytes: &[u8]) -> Option<(String, Vec<Arg>)> {
    let instructions = decode(bytes);
    for (index, instruction) in instructions.iter().enumerate() {
        if instruction.opcode != 0xbb {
            continue;
        }
        let created = class_name(&class.constants, instruction.index)?;
        let mut args = Vec::new();
        for candidate in &instructions[index + 1..] {
            if let Some(arg) = push_arg(class, candidate) {
                args.push(arg);
                continue;
            }
            match candidate.opcode {
                0xb7 => {
                    let (owner, name, _) = method_ref(class, candidate.index)?;
                    if owner == created && name == "<init>" {
                        return Some((created, args));
                    }
                }
                0x59 | 0x57 => {}
                _ => {}
            }
        }
    }
    None
}

fn push_arg(class: &ClassFile, instruction: &Ins) -> Option<Arg> {
    match instruction.opcode {
        0x02..=0x08 => Some(Arg::Int(instruction.opcode as i32 - 0x03)),
        0x0b..=0x0d => Some(Arg::Float((instruction.opcode - 0x0b) as f32)),
        0x0e..=0x0f => Some(Arg::Double((instruction.opcode - 0x0e) as f64)),
        0x10 | 0x11 => Some(Arg::Int(instruction.imm)),
        0x12 | 0x13 => match class.constants.get(instruction.index)? {
            Constant::Integer(value) => Some(Arg::Int(*value)),
            Constant::Float(value) => Some(Arg::Float(*value)),
            Constant::String(_) => {
                classfile::string_constant(&class.constants, instruction.index).map(Arg::Str)
            }
            _ => None,
        },
        0x14 => {
            // The class-file reader parks a Long/Double one slot past its
            // constant-pool index, so follow the `Unused` placeholder.
            let constant = match class.constants.get(instruction.index)? {
                Constant::Unused => class.constants.get(instruction.index + 1)?,
                other => other,
            };
            match constant {
                Constant::Double(value) => Some(Arg::Double(*value)),
                Constant::Long(value) => Some(Arg::Int(*value as i32)),
                _ => None,
            }
        }
        0xb2 => {
            let (owner, field, _) = field_ref(class, instruction.index)?;
            Some(Arg::Field(owner, field))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// BlockSetType / SoundType / SoundEvents
// ---------------------------------------------------------------------------

fn block_set_type_records(jar: &Jar) -> Result<BTreeMap<String, Value>, String> {
    let class = jar
        .class(BLOCK_SET_TYPE)
        .ok_or_else(|| "BlockSetType is not in the jar".to_string())?;
    let sound_types = sound_type_records(jar)?;
    let sound_ids = sound_event_ids(jar);
    let Some(code) = class
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
    else {
        return Err("BlockSetType has no initialiser".to_string());
    };
    let instructions = decode(&code.bytes);
    let mut out = BTreeMap::new();
    let mut index = 0usize;
    while index < instructions.len() {
        let instruction = &instructions[index];
        if instruction.opcode == 0xbb
            && class_name(&class.constants, instruction.index).as_deref() == Some(BLOCK_SET_TYPE)
        {
            let (args, end) = constructor_args(&class, &instructions, index);
            let args = expand_block_set_type_args(jar, &class, &args);
            if let Some((field, _)) = next_putstatic(&class, &instructions[end..], BLOCK_SET_TYPE) {
                if let Some(record) = block_set_type_record(&args, &sound_types, &sound_ids) {
                    out.insert(field, record);
                }
            }
            index = end;
            continue;
        }
        index += 1;
    }
    Ok(out)
}

/// The single-`String` `BlockSetType` constructor delegates to the 14-argument
/// one with the wooden defaults; expand `new BlockSetType("oak")` to that call.
fn expand_block_set_type_args(jar: &Jar, class: &ClassFile, args: &[Arg]) -> Vec<Arg> {
    if args.len() >= 14 {
        return args.to_vec();
    }
    let Some(name) = args.iter().find_map(|arg| match arg {
        Arg::Str(value) => Some(value.clone()),
        _ => None,
    }) else {
        return args.to_vec();
    };
    let Some(method) = class
        .methods
        .iter()
        .find(|method| method.name == "<init>" && method.descriptor == "(Ljava/lang/String;)V")
    else {
        return args.to_vec();
    };
    let Some(code) = method.code.as_ref() else {
        return args.to_vec();
    };
    let instructions = decode(&code.bytes);
    let mut out = vec![Arg::Str(name)];
    for instruction in &instructions {
        if let Some(arg) = push_arg(class, instruction) {
            out.push(arg);
            continue;
        }
        if instruction.opcode == 0xb7 {
            break;
        }
    }
    let _ = jar;
    out
}

fn block_set_type_record(
    args: &[Arg],
    sound_types: &BTreeMap<String, SoundTypeRecord>,
    sound_ids: &BTreeMap<String, String>,
) -> Option<Value> {
    if args.len() < 14 {
        return None;
    }
    let name = match &args[0] {
        Arg::Str(value) => value.clone(),
        _ => return None,
    };
    let flag = |slot: usize| match args.get(slot) {
        Some(Arg::Int(value)) => Some(*value != 0),
        _ => None,
    };
    let field_name = |slot: usize| match args.get(slot) {
        Some(Arg::Field(_, field)) => Some(field.clone()),
        _ => None,
    };
    let event = |slot: usize| {
        let field = field_name(slot)?;
        Some(Value::String(sound_ids.get(&field)?.clone()))
    };
    let pressure = Value::String(field_name(4)?);
    let sound_field = field_name(5)?;
    let sound = sound_types.get(&sound_field)?;
    let mut sound_object = Map::new();
    sound_object.insert("volume".to_string(), Value::from(sound.volume as f64));
    sound_object.insert("pitch".to_string(), Value::from(sound.pitch as f64));
    sound_object.insert("break".to_string(), Value::String(sound.break_id.clone()));
    sound_object.insert("place".to_string(), Value::String(sound.place_id.clone()));
    let mut object = Map::new();
    object.insert("name".to_string(), Value::String(name));
    object.insert("canOpenByHand".to_string(), Value::Bool(flag(1)?));
    object.insert("canOpenByWindCharge".to_string(), Value::Bool(flag(2)?));
    object.insert(
        "canButtonBeActivatedByArrows".to_string(),
        Value::Bool(flag(3)?),
    );
    object.insert("pressurePlateSensitivity".to_string(), pressure);
    object.insert("soundType".to_string(), Value::Object(sound_object));
    for (slot, key) in [
        (6, "doorClose"),
        (7, "doorOpen"),
        (8, "trapdoorClose"),
        (9, "trapdoorOpen"),
        (10, "pressurePlateClickOff"),
        (11, "pressurePlateClickOn"),
        (12, "buttonClickOff"),
        (13, "buttonClickOn"),
    ] {
        object.insert(key.to_string(), event(slot)?);
    }
    Some(Value::Object(object))
}

#[derive(Debug, Clone)]
struct SoundTypeRecord {
    volume: f32,
    pitch: f32,
    break_id: String,
    place_id: String,
}

fn sound_type_records(jar: &Jar) -> Result<BTreeMap<String, SoundTypeRecord>, String> {
    let class = jar
        .class(SOUND_TYPE)
        .ok_or_else(|| "SoundType is not in the jar".to_string())?;
    let sound_ids = sound_event_ids(jar);
    let Some(code) = class
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
    else {
        return Err("SoundType has no initialiser".to_string());
    };
    let instructions = decode(&code.bytes);
    let mut out = BTreeMap::new();
    let mut index = 0usize;
    while index < instructions.len() {
        let instruction = &instructions[index];
        if instruction.opcode == 0xbb
            && class_name(&class.constants, instruction.index).as_deref() == Some(SOUND_TYPE)
        {
            let (args, end) = constructor_args(&class, &instructions, index);
            if let Some((field, _)) = next_putstatic(&class, &instructions[end..], SOUND_TYPE) {
                if args.len() >= 7 {
                    let volume = match args[0] {
                        Arg::Float(value) => value,
                        _ => 0.0,
                    };
                    let pitch = match args[1] {
                        Arg::Float(value) => value,
                        _ => 0.0,
                    };
                    let break_field = match &args[2] {
                        Arg::Field(_, field) => field.clone(),
                        _ => continue,
                    };
                    let place_field = match &args[4] {
                        Arg::Field(_, field) => field.clone(),
                        _ => continue,
                    };
                    if let (Some(break_id), Some(place_id)) =
                        (sound_ids.get(&break_field), sound_ids.get(&place_field))
                    {
                        out.insert(
                            field,
                            SoundTypeRecord {
                                volume,
                                pitch,
                                break_id: break_id.clone(),
                                place_id: place_id.clone(),
                            },
                        );
                    }
                }
            }
            index = end;
            continue;
        }
        index += 1;
    }
    Ok(out)
}

/// `SoundEvents.<clinit>`: `field -> minecraft:<string>`.
fn sound_event_ids(jar: &Jar) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(class) = jar.class(SOUND_EVENTS) else {
        return out;
    };
    let Some(code) = class
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
    else {
        return out;
    };
    let mut pending: Option<String> = None;
    for instruction in decode(&code.bytes) {
        match instruction.opcode {
            0x12 | 0x13 => {
                if let Some(text) = classfile::string_constant(&class.constants, instruction.index)
                {
                    pending = Some(text);
                }
            }
            0xb3 => {
                let Some((owner, field, _)) = field_ref(&class, instruction.index) else {
                    continue;
                };
                if owner != SOUND_EVENTS {
                    continue;
                }
                if let Some(text) = pending.take() {
                    out.insert(
                        field,
                        if text.starts_with("minecraft:") {
                            text
                        } else {
                            format!("minecraft:{text}")
                        },
                    );
                }
            }
            _ => {}
        }
    }
    out
}

/// Arguments of the constructor invoked by the `new` at `start`.
fn constructor_args(class: &ClassFile, instructions: &[Ins], start: usize) -> (Vec<Arg>, usize) {
    let mut args = Vec::new();
    for (offset, candidate) in instructions[start + 1..].iter().enumerate() {
        if let Some(arg) = push_arg(class, candidate) {
            args.push(arg);
            continue;
        }
        if candidate.opcode == 0xb7 {
            return (args, start + 1 + offset + 1);
        }
    }
    (args, instructions.len())
}

fn next_putstatic(class: &ClassFile, instructions: &[Ins], owner: &str) -> Option<(String, usize)> {
    for (offset, instruction) in instructions.iter().enumerate() {
        if instruction.opcode != 0xb3 {
            continue;
        }
        let (actual, field, _) = field_ref(class, instruction.index)?;
        if actual != owner {
            continue;
        }
        return Some((field, offset));
    }
    None
}

// ---------------------------------------------------------------------------
// Per-block values
// ---------------------------------------------------------------------------

fn growth_probability(jar: &Jar, internal: &str) -> Option<f64> {
    let class = jar.class(internal)?;
    for method in &class.methods {
        if method.name != "<init>" || !method.descriptor.starts_with("(L") {
            continue;
        }
        let Some(code) = method.code.as_ref() else {
            continue;
        };
        let instructions = decode(&code.bytes);
        let mut doubles = Vec::new();
        for instruction in &instructions {
            if let Some(Arg::Double(value)) = push_arg(&class, instruction) {
                doubles.push(value);
            }
            if instruction.opcode == 0xb7 {
                let Some((_, _, descriptor)) = method_ref(&class, instruction.index) else {
                    continue;
                };
                if descriptor.contains("D)") {
                    if let Some(value) = doubles.last() {
                        return Some(*value);
                    }
                }
            }
        }
    }
    None
}

fn bush_shape_height(jar: &Jar, internal: &str) -> Option<i32> {
    let class = jar.class(internal)?;
    for method in &class.methods {
        if method.name != "<init>" || !method.descriptor.starts_with("(L") {
            continue;
        }
        let Some(code) = method.code.as_ref() else {
            continue;
        };
        let instructions = decode(&code.bytes);
        let mut ints = Vec::new();
        for instruction in &instructions {
            if let Some(Arg::Int(value)) = push_arg(&class, instruction) {
                ints.push(value);
            }
            if instruction.opcode == 0xb7 {
                let Some((owner, name, descriptor)) = method_ref(&class, instruction.index) else {
                    continue;
                };
                if owner == internal && name == "<init>" && descriptor.ends_with("I)V") {
                    if let Some(value) = ints.last() {
                        return Some(*value);
                    }
                }
            }
        }
    }
    None
}

fn multiface_support(jar: &Jar, internal: &str) -> Option<fn(&str) -> bool> {
    // The three registry multiface blocks do not override `isFaceSupported`.
    let mut current = internal.to_string();
    for _ in 0..64 {
        if current == MULTIFACE {
            break;
        }
        let class = jar.class(&current)?;
        if let Some(method) = class
            .methods
            .iter()
            .find(|method| method.name == "isFaceSupported")
        {
            let code = method.code.as_ref()?;
            let instructions = decode(&code.bytes);
            let all = instructions
                .iter()
                .find(|instruction| instruction.opcode == 0x04)
                .is_some();
            let none = instructions
                .iter()
                .find(|instruction| instruction.opcode == 0x03)
                .is_some();
            if all {
                return Some(face_always);
            }
            if none {
                return Some(face_never);
            }
            return None;
        }
        if class.super_name.is_empty() {
            return None;
        }
        current = class.super_name.clone();
    }
    Some(face_always)
}

fn face_always(_: &str) -> bool {
    true
}

fn face_never(_: &str) -> bool {
    false
}

fn hopper_interaction_shapes(jar: &Jar, internal: &str) -> Option<Value> {
    let class = jar.class(internal)?;
    let code = class.method("<init>")?.code.clone()?;
    let instructions = decode(&code.bytes);
    let mut start = 0usize;
    let mut end = instructions.len();
    for (index, instruction) in instructions.iter().enumerate() {
        if instruction.opcode == 0xb8 || instruction.opcode == 0xb6 {
            let Some((_, name, _)) = method_ref(&class, instruction.index) else {
                continue;
            };
            if name == "builderWithExpectedSize" || name == "builder" {
                start = index;
            }
        }
        if instruction.opcode == 0xb5 {
            let Some((_, field, _)) = field_ref(&class, instruction.index) else {
                continue;
            };
            if field == "interactionShapes" {
                end = index;
            }
        }
    }
    let mut directions: BTreeSet<String> = BTreeSet::new();
    let mut rotate = false;
    for instruction in &instructions[start..end] {
        if instruction.opcode == 0xb6 || instruction.opcode == 0xb8 {
            let Some((_, name, _)) = method_ref(&class, instruction.index) else {
                continue;
            };
            if name == "rotateHorizontal" || name == "rotateAll" {
                rotate = true;
            }
        }
        if instruction.opcode == 0xb2 {
            let Some((owner, field, _)) = field_ref(&class, instruction.index) else {
                continue;
            };
            if owner.ends_with("/Direction") && DIRECTIONS.contains(&field.as_str()) {
                directions.insert(field);
            }
        }
    }
    if rotate {
        for direction in ["NORTH", "SOUTH", "WEST", "EAST"] {
            directions.insert(direction.to_string());
        }
    }
    if directions.is_empty() {
        return None;
    }
    let mut object = Map::new();
    for direction in directions {
        object.insert(direction, Value::String("<VoxelShape>".to_string()));
    }
    Some(Value::Object(object))
}

fn direction_shape_map(directions: &[&str]) -> Value {
    let mut object = Map::new();
    for direction in directions {
        object.insert(
            direction.to_string(),
            Value::String("<VoxelShape>".to_string()),
        );
    }
    Value::Object(object)
}

fn redstone_cross_state(jar: &Jar, internal: &str, table: &states::StateTable) -> Option<u32> {
    let class = jar.class(internal)?;
    let code = class.method("<init>")?.code.clone()?;
    let instructions = decode(&code.bytes);
    let mut pending: Vec<Arg> = Vec::new();
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    for instruction in &instructions {
        if let Some(arg) = push_arg(&class, instruction) {
            pending.push(arg);
            continue;
        }
        if instruction.opcode == 0xb6 {
            let Some((_, name, _)) = method_ref(&class, instruction.index) else {
                continue;
            };
            if name != "setValue" {
                continue;
            }
            if pending.len() >= 2 {
                let property = pending.iter().rev().nth(1)?;
                let value = pending.last()?;
                if let Arg::Field(_, field) = property {
                    let key = field.to_lowercase();
                    let text = match value {
                        Arg::Int(number) => Some(number.to_string()),
                        Arg::Field(_, constant) => Some(constant.to_lowercase()),
                        _ => None,
                    };
                    if let Some(text) = text {
                        values.insert(key, text);
                    }
                }
            }
            pending.clear();
        }
    }
    let block = table
        .blocks
        .iter()
        .find(|block| block.id == "minecraft:redstone_wire")?;
    let mut base = 0u32;
    for candidate in &table.blocks {
        if candidate.id == block.id {
            break;
        }
        base += candidate.state_count as u32;
    }
    let mut properties = block.properties.clone();
    properties.sort_by(|left, right| left.name.cmp(&right.name));
    let mut index = 0u32;
    let mut radix = 1u32;
    for property in properties.iter().rev() {
        let value = values.get(&property.name)?;
        let position = property
            .values
            .iter()
            .position(|candidate| candidate == value)? as u32;
        index += position * radix;
        radix *= property.values.len().max(1) as u32;
    }
    Some(base + index)
}

fn redstone_evaluator(jar: &Jar, internal: &str) -> Option<Value> {
    let class = jar.class(internal)?;
    let code = class.method("<init>")?.code.clone()?;
    let instructions = decode(&code.bytes);
    let mut created: Option<String> = None;
    for instruction in &instructions {
        if instruction.opcode == 0xbb {
            if let Some(name) = class_name(&class.constants, instruction.index) {
                created = Some(name);
            }
        }
        if instruction.opcode == 0xb5 {
            let Some((_, field, _)) = field_ref(&class, instruction.index) else {
                continue;
            };
            if field == "evaluator" {
                let name = created?;
                return Some(Value::String(format!("<{}>", simple(&name))));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Bytecode helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Ins {
    opcode: u8,
    index: usize,
    imm: i32,
}

fn decode(bytes: &[u8]) -> Vec<Ins> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let opcode = bytes[at];
        let mut index = 0usize;
        let mut imm = 0i32;
        let length = match opcode {
            0x10 => {
                imm = *bytes.get(at + 1).unwrap_or(&0) as i8 as i32;
                2
            }
            0x11 => {
                imm = i16::from_be_bytes([
                    *bytes.get(at + 1).unwrap_or(&0),
                    *bytes.get(at + 2).unwrap_or(&0),
                ]) as i32;
                3
            }
            0x12 => {
                index = *bytes.get(at + 1).unwrap_or(&0) as usize;
                2
            }
            0x13 | 0x14 | 0xb2..=0xb8 | 0xbb | 0xbd | 0xc0 | 0xc1 => {
                index = u16::from_be_bytes([
                    *bytes.get(at + 1).unwrap_or(&0),
                    *bytes.get(at + 2).unwrap_or(&0),
                ]) as usize;
                3
            }
            0xb9 | 0xba => {
                index = u16::from_be_bytes([
                    *bytes.get(at + 1).unwrap_or(&0),
                    *bytes.get(at + 2).unwrap_or(&0),
                ]) as usize;
                5
            }
            0x15..=0x19 | 0x36..=0x3a | 0xa9 | 0xbc => {
                index = *bytes.get(at + 1).unwrap_or(&0) as usize;
                2
            }
            0xc4 => {
                // wide: the following opcode decides the width.
                match bytes.get(at + 1).copied().unwrap_or(0) {
                    0x84 => 6,
                    _ => 4,
                }
            }
            0x84 => 3,
            0x99..=0xa8 | 0xc6 | 0xc7 => 3,
            0xc5 => 4,
            0xc8 | 0xc9 => 5,
            0xaa => {
                // tableswitch: pad to a 4-byte boundary, then 3 ints + entries.
                let base = at + 1;
                let pad = (4 - (base % 4)) % 4;
                let low = i32::from_be_bytes([
                    *bytes.get(base + pad + 4).unwrap_or(&0),
                    *bytes.get(base + pad + 5).unwrap_or(&0),
                    *bytes.get(base + pad + 6).unwrap_or(&0),
                    *bytes.get(base + pad + 7).unwrap_or(&0),
                ]);
                let high = i32::from_be_bytes([
                    *bytes.get(base + pad + 8).unwrap_or(&0),
                    *bytes.get(base + pad + 9).unwrap_or(&0),
                    *bytes.get(base + pad + 10).unwrap_or(&0),
                    *bytes.get(base + pad + 11).unwrap_or(&0),
                ]);
                let count = (high - low + 1).max(0) as usize;
                1 + pad + 12 + count * 4
            }
            0xab => {
                let base = at + 1;
                let pad = (4 - (base % 4)) % 4;
                let count = i32::from_be_bytes([
                    *bytes.get(base + pad + 4).unwrap_or(&0),
                    *bytes.get(base + pad + 5).unwrap_or(&0),
                    *bytes.get(base + pad + 6).unwrap_or(&0),
                    *bytes.get(base + pad + 7).unwrap_or(&0),
                ])
                .max(0) as usize;
                1 + pad + 8 + count * 8
            }
            _ => 1,
        };
        out.push(Ins { opcode, index, imm });
        at += length;
    }
    out
}

fn method_ref(class: &ClassFile, index: usize) -> Option<(String, String, String)> {
    classfile::method_ref(&class.constants, index)
}

fn field_ref(class: &ClassFile, index: usize) -> Option<(String, String, String)> {
    classfile::field_ref(&class.constants, index)
}

fn class_name(constants: &[Constant], index: usize) -> Option<String> {
    let Constant::Class(reference) = constants.get(index)? else {
        return None;
    };
    let inner: usize = reference.trim_start_matches('#').parse().ok()?;
    match constants.get(inner)? {
        Constant::Utf8(name) => Some(name.clone()),
        _ => None,
    }
}

fn indy_target(class: &ClassFile, index: usize) -> Option<(String, String, String)> {
    let Constant::InvokeDynamic { bootstrap, .. } = class.constants.get(index)? else {
        return None;
    };
    let entry = class.bootstraps.get(*bootstrap)?;
    for argument in &entry.arguments {
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
        let owner_name = class_name(&class.constants, owner_index)?;
        let Constant::NameAndType { name, kind } = class.constants.get(name_and_type)? else {
            continue;
        };
        let name_index: usize = name.trim_start_matches('#').parse().ok()?;
        let kind_index: usize = kind.trim_start_matches('#').parse().ok()?;
        return Some((
            owner_name,
            classfile::utf8(&class.constants, name_index),
            classfile::utf8(&class.constants, kind_index),
        ));
    }
    None
}

struct Jar {
    files: BTreeMap<String, Vec<u8>>,
    parsed: RefCell<BTreeMap<String, Option<Rc<ClassFile>>>>,
}

impl Jar {
    fn open(path: &Path) -> Result<Jar, String> {
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
            if let Some(stem) = name.strip_suffix(".class") {
                let mut data = Vec::new();
                entry
                    .read_to_end(&mut data)
                    .map_err(|error| format!("{name}: {error}"))?;
                files.insert(stem.to_string(), data);
            }
        }
        Ok(Jar {
            files,
            parsed: RefCell::new(BTreeMap::new()),
        })
    }

    fn resolve(&self, simple: &str) -> Option<String> {
        let direct = format!("net/minecraft/world/level/block/{simple}");
        if self.files.contains_key(&direct) {
            return Some(direct);
        }
        self.files
            .keys()
            .find(|name| {
                name.starts_with("net/minecraft/world/level/block/")
                    && name.ends_with(&format!("/{simple}"))
            })
            .cloned()
    }

    fn access(&self, internal: &str) -> Option<u16> {
        let bytes = self.files.get(internal)?;
        if bytes.len() < 10 {
            return None;
        }
        Some(u16::from_be_bytes([bytes[8], bytes[9]]))
    }

    fn class(&self, internal: &str) -> Option<Rc<ClassFile>> {
        if let Some(hit) = self.parsed.borrow().get(internal) {
            return hit.clone();
        }
        let parsed = self
            .files
            .get(internal)
            .and_then(|bytes| classfile::parse(bytes).ok())
            .map(Rc::new);
        self.parsed
            .borrow_mut()
            .insert(internal.to_string(), parsed.clone());
        parsed
    }
}
