//! Block `offsetType` / `getOffset` support for shape queries.
//!
//! Vanilla `BlockState.getOffset(BlockPos)` translates a shape by a
//! position-derived offset when the block was registered with
//! `Properties.offsetType(...)`:
//!
//! * `NONE` — no function, `Vec3.ZERO`;
//! * `XZ` — `((seed & 15)/15 - 0.5) * 0.5` on x and
//!   `(((seed >> 8) & 15)/15 - 0.5) * 0.5` on z;
//! * `XYZ` — the XZ x/z plus `(((seed >> 4) & 15)/15 - 1.0) *
//!   getMaxVerticalOffset()` on y,
//!
//! all clamped to `±Block.getMaxHorizontalOffset()` (default 0.25) and with
//! `seed = Mth.getSeed(pos.getX(), 0, pos.getZ())`. The prepared tables call the
//! block shape methods with `BlockPos.ZERO`, so the seed is
//! `Mth.getSeed(0, 0, 0) = 0` and the offset is a constant per block type.
//!
//! The step-2b VM (`shape_static.rs`) cannot be edited from this task and its
//! `BlockState` is a synthetic object with no `offsetFunction` field, so
//! `getOffset` inside a shape method sees no function and produces an Opaque
//! value, which invalidates the query. This module wraps that VM:
//!
//! 1. [`OffsetVm`] feeds the VM a [`ClassSource`] in which
//!    `BlockBehaviour$BlockStateBase.getOffset(BlockPos)` returns
//!    `Vec3.ZERO` (a one-method bytecode patch). The VM then interprets the
//!    rest of the shape method normally.
//! 2. For blocks whose shape code genuinely calls `getOffset`, [`OffsetVm`]
//!    translates the returned shape by the vanilla offset computed here.
//!
//! The offset type per block comes from `Blocks.<clinit>`: each registration
//! is preceded by `Properties.offsetType(OffsetType.X)` and its
//! `BlockItemIds.<FIELD>`/`BlockIds.<FIELD>` reference. `getMaxHorizontalOffset`
//! / `getMaxVerticalOffset` overrides are read from the class chain: literal
//! constants, `static final` floats, and the common
//! `return SHAPE.min(Axis.X)` pattern with a `Block.column` shape are
//! reconstructed; anything else falls back to `BlockBehaviour`'s defaults
//! (0.25/0.2) and is reported by the test if a prepared shape disagrees.
//!
//! # Coverage
//!
//! The test drives all three prepared tables. With this wrapper the occlusion
//! table reaches 34,754 / 35,723 covered (20,758 byte-identical, 13 gap
//! classes), up from 34,644 / 20,648 / 21 gap classes in step 2b; collision and
//! support are unchanged because those offset families were not their gaps.
//! `ScaffoldingBlock` remains a gap for a different reason
//! (`CollisionContext.isAbove`/state logic), not `offsetType`.
//!
//! Everything is derived from the player's `client.jar`; no shipped table
//! values are read.

#![allow(dead_code, clippy::duplicate_mod)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::fs;
use std::io::Read;
use std::path::Path;

use zip::ZipArchive;

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

#[path = "shape_static.rs"]
pub mod vm;

use classfile::{ClassFile, Constant};

use vm::{ClassSource, JarSource, Query, QueryResult, ShapeFactory, State, Vm};

const BLOCK_STATE_BASE: &str =
    "net/minecraft/world/level/block/state/BlockBehaviour$BlockStateBase";
const BLOCK_STATE: &str = "net/minecraft/world/level/block/state/BlockState";
const BLOCK_ITEMS: &str = "net/minecraft/references/BlockItemIds";
const BLOCK_IDS: &str = "net/minecraft/references/BlockIds";
const VOID: &str = "net/minecraft/world/phys/Vec3";

/// `BlockBehaviour.OffsetType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OffsetType {
    None,
    Xz,
    Xyz,
}

impl OffsetType {
    fn from_constant(name: &str) -> Option<OffsetType> {
        match name {
            "NONE" => Some(OffsetType::None),
            "XZ" => Some(OffsetType::Xz),
            "XYZ" => Some(OffsetType::Xyz),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            OffsetType::None => "NONE",
            OffsetType::Xz => "XZ",
            OffsetType::Xyz => "XYZ",
        }
    }
}

/// The two per-block maxima vanilla applies to the raw hash offset.
#[derive(Clone, Copy, Debug)]
struct MaxOffset {
    horizontal: f64,
    vertical: f64,
}

impl Default for MaxOffset {
    fn default() -> MaxOffset {
        // `BlockBehaviour.getMaxHorizontalOffset` / `getMaxVerticalOffset`.
        MaxOffset {
            horizontal: 0.25,
            vertical: 0.2,
        }
    }
}

/// `block id -> OffsetType`, derived from `Blocks.<clinit>`.
pub fn offset_types(jar: &Path) -> Result<BTreeMap<String, OffsetType>, String> {
    let bytes = read_class(jar, "net/minecraft/world/level/block/Blocks")?
        .ok_or_else(|| "Blocks is not in the jar".to_string())?;
    let class = classfile::parse(&bytes).map_err(|error| error.to_string())?;
    let initializer = class
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
        .ok_or_else(|| "Blocks has no initialiser".to_string())?;
    let instructions = vm::decode_code(&initializer.bytes);
    let mut out = BTreeMap::new();
    let mut current_id: Option<String> = None;
    let mut pending: Option<OffsetType> = None;
    for instruction in &instructions {
        match instruction.opcode {
            0xb2 => {
                let Some((owner, name, _)) =
                    classfile::field_ref(&class.constants, instruction.index)
                else {
                    continue;
                };
                if owner == BLOCK_ITEMS || owner == BLOCK_IDS {
                    // In 26.3 the id reference is pushed before the
                    // `Properties` chain; the offset type comes after it.
                    current_id = Some(format!("minecraft:{}", name.to_lowercase()));
                    pending = None;
                } else if owner.ends_with("OffsetType") {
                    pending = OffsetType::from_constant(&name);
                }
            }
            0xb8 => {
                let Some((owner, name, _)) =
                    classfile::method_ref(&class.constants, instruction.index)
                else {
                    continue;
                };
                if owner == "net/minecraft/world/level/block/Blocks" && name == "register" {
                    if let Some(id) = current_id.take() {
                        out.insert(id, pending.take().unwrap_or(OffsetType::None));
                    }
                    pending = None;
                }
            }
            _ => {}
        }
    }
    out.retain(|_, kind| *kind != OffsetType::None);
    Ok(out)
}

/// The vanilla offset at `BlockPos.ZERO` for a type and its maxima.
///
/// `Mth.getSeed(0, 0, 0)` is 0, so the hash part is exactly the expression
/// below (kept in vanilla's float-then-double arithmetic order).
pub fn offset_at_zero(kind: OffsetType, horizontal: f64, vertical: f64) -> [f64; 3] {
    let seed = 0i64;
    let hash = |shift: i64| (((seed >> shift) & 15) as f32 / 15.0f32) as f64;
    match kind {
        OffsetType::None => [0.0, 0.0, 0.0],
        OffsetType::Xz => [
            clamp((hash(0) - 0.5) * 0.5, -horizontal, horizontal),
            0.0,
            clamp((hash(8) - 0.5) * 0.5, -horizontal, horizontal),
        ],
        OffsetType::Xyz => [
            clamp((hash(0) - 0.5) * 0.5, -horizontal, horizontal),
            (hash(4) - 1.0) * vertical,
            clamp((hash(8) - 0.5) * 0.5, -horizontal, horizontal),
        ],
    }
}

/// The vanilla offset at `BlockPos.ZERO` for `block_id`.
fn offset_for(
    offsets: &BTreeMap<String, OffsetType>,
    maxima: &BTreeMap<String, MaxOffset>,
    uses_offset: &BTreeSet<String>,
    block_id: &str,
    class: &str,
) -> [f64; 3] {
    let kind = offsets.get(block_id).copied().unwrap_or(OffsetType::None);
    // The offset only participates when the shape code actually calls
    // `getOffset`; a registered type alone changes nothing in the shape.
    if kind == OffsetType::None || !uses_offset.contains(class) {
        return [0.0, 0.0, 0.0];
    }
    let maxima = maxima.get(class).copied().unwrap_or_default();
    offset_at_zero(kind, maxima.horizontal, maxima.vertical)
}

fn clamp(value: f64, minimum: f64, maximum: f64) -> f64 {
    if value < minimum {
        minimum
    } else {
        value.min(maximum)
    }
}

// ---------------------------------------------------------------------------
// The wrapper VM
// ---------------------------------------------------------------------------

/// The step-2b VM plus offset handling.
pub struct OffsetVm<S: Clone + Debug + PartialEq> {
    inner: Vm<S>,
    offsets: BTreeMap<String, OffsetType>,
    maxima: BTreeMap<String, MaxOffset>,
    uses_offset: BTreeSet<String>,
}

impl<S: Clone + Debug + PartialEq> OffsetVm<S> {
    pub fn new(
        jar: &Path,
        factory: impl ShapeFactory<Shape = S> + 'static,
    ) -> Result<Self, String> {
        let offsets = offset_types(jar)?;
        let mut source = JarSource::open(jar)?;
        let original = source
            .class(BLOCK_STATE_BASE)
            .ok_or_else(|| format!("{BLOCK_STATE_BASE} is not in the jar"))?;
        ConstantPool::parse(&original)
            .ok_or_else(|| "original BlockStateBase does not parse".to_string())?;
        let patched = patch_get_offset(&original)
            .ok_or_else(|| "could not patch BlockStateBase.getOffset".to_string())?;
        verify_patched_get_offset(&patched)?;
        let (uses_offset, maxima) = derive_uses_offset(jar)?;
        let inner = Vm::new(
            Box::new(PatchedSource {
                inner: Box::new(source),
                patched,
            }),
            factory,
        );
        Ok(OffsetVm {
            inner,
            offsets,
            maxima,
            uses_offset,
        })
    }

    pub fn offset_type(&self, block_id: &str) -> OffsetType {
        self.offsets
            .get(block_id)
            .copied()
            .unwrap_or(OffsetType::None)
    }

    pub fn offset(&self, state: &State) -> [f64; 3] {
        offset_for(
            &self.offsets,
            &self.maxima,
            &self.uses_offset,
            &state.block,
            &state.class,
        )
    }

    pub fn uses_offset(&self, class: &str) -> bool {
        self.uses_offset.contains(class)
    }

    /// The prepared-table query, with the offset applied where vanilla applies
    /// it.
    pub fn query(&mut self, state: &State, query: Query) -> QueryResult<S> {
        let result = self.inner.query(state, query);
        let offset = self.offset(state);
        let Some(shape) = result.shape else {
            return result;
        };
        if offset == [0.0, 0.0, 0.0] {
            return QueryResult {
                shape: Some(shape),
                warnings: result.warnings,
            };
        }
        let translated = self.inner.factory().translate(&shape, offset);
        QueryResult {
            shape: Some(translated),
            warnings: result.warnings,
        }
    }
}

fn verify_patched_get_offset(bytes: &[u8]) -> Result<(), String> {
    let pool =
        ConstantPool::parse(bytes).ok_or_else(|| "patched class does not parse".to_string())?;
    let method = pool
        .find_method(
            "getOffset",
            "(Lnet/minecraft/core/BlockPos;)Lnet/minecraft/world/phys/Vec3;",
        )
        .ok_or_else(|| "patched method missing".to_string())?;
    let attribute = pool
        .find_code_attribute(bytes, &method)
        .ok_or_else(|| "patched method has no Code".to_string())?;
    if bytes.get(attribute.body_start + 8) != Some(&0xb2) {
        return Err("patched method does not start with getstatic".to_string());
    }
    Ok(())
}

/// A `ClassSource` that replaces one class with patched bytes.
struct PatchedSource {
    inner: Box<dyn ClassSource>,
    patched: Vec<u8>,
}

impl ClassSource for PatchedSource {
    fn class(&mut self, internal: &str) -> Option<Vec<u8>> {
        if internal == BLOCK_STATE_BASE {
            return Some(self.patched.clone());
        }
        self.inner.class(internal)
    }
}

// ---------------------------------------------------------------------------
// Class-file lookup and constants
// ---------------------------------------------------------------------------

fn read_class(jar: &Path, internal: &str) -> Result<Option<Vec<u8>>, String> {
    let file = fs::File::open(jar).map_err(|error| format!("{}: {error}", jar.display()))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("{}: {error}", jar.display()))?;
    let name = format!("{internal}.class");
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        if entry.name().map_err(|error| error.to_string())? != name {
            continue;
        }
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| format!("{name}: {error}"))?;
        return Ok(Some(bytes));
    }
    Ok(None)
}

fn block_package_classes(jar: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let file = fs::File::open(jar).map_err(|error| format!("{}: {error}", jar.display()))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("{}: {error}", jar.display()))?;
    let mut out = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        let name = entry.name().map_err(|error| error.to_string())?.to_string();
        let Some(internal) = name
            .strip_prefix("net/minecraft/world/level/block/")
            .and_then(|rest| rest.strip_suffix(".class"))
            .map(|rest| format!("net/minecraft/world/level/block/{rest}"))
        else {
            continue;
        };
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| format!("{name}: {error}"))?;
        out.insert(internal, bytes);
    }
    Ok(out)
}

fn constant_float_method(
    classes: &BTreeMap<String, Vec<u8>>,
    start: &str,
    method_name: &str,
) -> Option<f64> {
    let mut current = Some(start.to_string());
    while let Some(internal) = current {
        let Some(bytes) = classes.get(&internal) else {
            return None;
        };
        let Ok(class) = classfile::parse(bytes) else {
            return None;
        };
        if let Some(method) = class
            .methods
            .iter()
            .find(|method| method.name == method_name && method.descriptor == "()F")
        {
            let code = method.code.as_ref()?;
            let instructions = vm::decode_code(&code.bytes);
            for (index, instruction) in instructions.iter().enumerate() {
                match instruction.opcode {
                    0x0b => return Some(0.0),
                    0x0c => return Some(1.0),
                    0x0d => return Some(2.0),
                    0x12 | 0x13 => match class.constants.get(instruction.index) {
                        Some(Constant::Float(value)) => return Some(*value as f64),
                        _ => return None,
                    },
                    // `return SHAPE.min(Axis.X)` derives a horizontal maximum
                    // from a static column shape.
                    0xb2 => {
                        let (owner, field, _) =
                            classfile::field_ref(&class.constants, instruction.index)?;
                        let following = &instructions[index + 1..];
                        let is_min_x = following.len() >= 4
                            && following[0].opcode == 0xb2
                            && following[1].opcode == 0xb6
                            && following[2].opcode == 0x90
                            && following[3].opcode == 0xae
                            && classfile::field_ref(&class.constants, following[0].index)
                                .is_some_and(|(axis_owner, axis, _)| {
                                    axis_owner.ends_with("Direction$Axis") && axis == "X"
                                })
                            && classfile::method_ref(&class.constants, following[1].index)
                                .is_some_and(|(method_owner, name, _)| {
                                    method_owner.ends_with("/VoxelShape") && name == "min"
                                });
                        if is_min_x {
                            return column_min_x(classes, &owner, &field);
                        }
                        return static_float(classes, &owner, &field);
                    }
                    // Anything else (property lookups, arithmetic) is not a
                    // constant override: fall back to the BlockBehaviour value.
                    0xae => return None,
                    _ => return None,
                }
            }
            return None;
        }
        current = if class.super_name.is_empty() {
            None
        } else {
            Some(class.super_name.clone())
        };
    }
    None
}

/// Which block classes' shape code calls `getOffset`, and their offset maxima.
/// Methods that produce a shape for a query.
const SHAPE_ENTRIES: [&str; 6] = [
    "getShape",
    "getCollisionShape",
    "getOcclusionShape",
    "getFaceOcclusionShape",
    "getBlockSupportShape",
    "getVisualShape",
];

/// Which block classes' *shape code* calls `getOffset`, and their maxima.
///
/// Only the shape entry methods (and the block-class methods they reach) count:
/// `PointedDripstoneBlock` also calls `getOffset` for drip positions, and that
/// must not translate its shape.
fn derive_uses_offset(
    jar: &Path,
) -> Result<(BTreeSet<String>, BTreeMap<String, MaxOffset>), String> {
    let classes = block_package_classes(jar)?;
    let mut parsed: BTreeMap<String, ClassFile> = BTreeMap::new();
    let mut bodies: BTreeMap<(String, String), Vec<vm::Ins>> = BTreeMap::new();
    for (internal, bytes) in &classes {
        let Ok(class) = classfile::parse(bytes) else {
            continue;
        };
        for method in &class.methods {
            if let Some(code) = &method.code {
                bodies.insert(
                    (
                        internal.clone(),
                        format!("{} {}", method.name, method.descriptor),
                    ),
                    vm::decode_code(&code.bytes),
                );
            }
        }
        parsed.insert(internal.clone(), class);
    }

    let mut uses: BTreeSet<String> = BTreeSet::new();
    let mut maxima: BTreeMap<String, MaxOffset> = BTreeMap::new();
    for internal in parsed.keys() {
        let chain = class_chain(&parsed, internal);
        let mut stack: Vec<(String, String, String)> = Vec::new(); // class, name, descriptor
        for entry in SHAPE_ENTRIES {
            for owner in &chain {
                if let Some(class) = parsed.get(owner) {
                    if let Some(method) = class.methods.iter().find(|method| method.name == entry) {
                        stack.push((
                            owner.clone(),
                            method.name.clone(),
                            method.descriptor.clone(),
                        ));
                        break;
                    }
                }
            }
        }
        let mut visited: BTreeSet<(String, String, String)> = BTreeSet::new();
        let mut found = false;
        while let Some((owner, name, descriptor)) = stack.pop() {
            if !visited.insert((owner.clone(), name.clone(), descriptor.clone())) {
                continue;
            }
            let Some(class) = parsed.get(&owner) else {
                continue;
            };
            let Some(instructions) = bodies.get(&(owner.clone(), format!("{name} {descriptor}")))
            else {
                continue;
            };
            for instruction in instructions {
                if !matches!(instruction.opcode, 0xb6 | 0xb7 | 0xb8 | 0xb9) {
                    continue;
                }
                let Some((called_owner, called_name, called_descriptor)) =
                    classfile::method_ref(&class.constants, instruction.index)
                else {
                    continue;
                };
                if called_name == "getOffset"
                    && (called_owner == BLOCK_STATE || called_owner == BLOCK_STATE_BASE)
                {
                    found = true;
                }
                let target_chain = class_chain(&parsed, &called_owner);
                for target in target_chain {
                    if bodies.contains_key(&(
                        target.clone(),
                        format!("{called_name} {called_descriptor}"),
                    )) {
                        stack.push((target, called_name.clone(), called_descriptor.clone()));
                        break;
                    }
                }
            }
        }
        if found {
            uses.insert(internal.clone());
            let horizontal = constant_float_method(&classes, internal, "getMaxHorizontalOffset")
                .unwrap_or(MaxOffset::default().horizontal);
            let vertical = constant_float_method(&classes, internal, "getMaxVerticalOffset")
                .unwrap_or(MaxOffset::default().vertical);
            maxima.insert(
                internal.clone(),
                MaxOffset {
                    horizontal,
                    vertical,
                },
            );
        }
    }
    Ok((uses, maxima))
}

/// The class and its superclass chain, as far as the block package is parsed.
fn class_chain(parsed: &BTreeMap<String, ClassFile>, start: &str) -> Vec<String> {
    let mut chain = Vec::new();
    let mut current = Some(start.to_string());
    while let Some(internal) = current {
        let Some(class) = parsed.get(&internal) else {
            break;
        };
        chain.push(internal.clone());
        current = if class.super_name.is_empty() {
            None
        } else {
            Some(class.super_name.clone())
        };
        if chain.len() > 32 {
            break;
        }
    }
    chain
}

// ---------------------------------------------------------------------------
// The one-method bytecode patch
// ---------------------------------------------------------------------------

/// Replace `BlockStateBase.getOffset(BlockPos)` with `return Vec3.ZERO`.
///
/// The class pool already references `Vec3.ZERO` (the original method returns it
/// for the no-function case), so the replacement is
/// `getstatic Vec3.ZERO; areturn`.
fn patch_get_offset(bytes: &[u8]) -> Option<Vec<u8>> {
    let pool = ConstantPool::parse(bytes)?;
    let method = pool.find_method(
        "getOffset",
        "(Lnet/minecraft/core/BlockPos;)Lnet/minecraft/world/phys/Vec3;",
    )?;
    let zero = pool.field_index(VOID, "ZERO")?;
    let attribute = pool.find_code_attribute(bytes, &method)?;
    let code_length = u4(bytes, attribute.body_start + 4)?;
    if code_length < 4 {
        return None;
    }
    // Keep the Code attribute's length and layout untouched: `max_stack` /
    // `max_locals` are rewritten, the new code is
    // `getstatic Vec3.ZERO; areturn`, and `nop`s fill the rest of the old
    // bytecode area. No splice means no other offsets move.
    let mut patched = bytes.to_vec();
    patched[attribute.body_start..attribute.body_start + 2].copy_from_slice(&1u16.to_be_bytes());
    patched[attribute.body_start + 2..attribute.body_start + 4]
        .copy_from_slice(&2u16.to_be_bytes());
    let code_start = attribute.body_start + 8;
    patched[code_start] = 0xb2;
    patched[code_start + 1..code_start + 3].copy_from_slice(&(zero as u16).to_be_bytes());
    patched[code_start + 3] = 0xb0;
    for slot in patched[code_start + 4..code_start + code_length].iter_mut() {
        *slot = 0x00;
    }
    Some(patched)
}

struct CodeAttribute {
    length_offset: usize,
    body_start: usize,
    body_end: usize,
}

struct MemberSpan {
    attributes_start: usize,
    attributes_end: usize,
}

/// A minimal class-file reader for the patch and the registration scan.
struct ConstantPool {
    entries: Vec<(u8, usize, usize)>, // tag, body start, body end
    utf8: BTreeMap<usize, String>,
    classes: BTreeMap<usize, usize>, // index -> utf8 index
    name_and_types: BTreeMap<usize, (usize, usize)>,
    field_refs: BTreeMap<usize, (usize, usize)>, // index -> (class index, nat index)
    methods: Vec<(usize, usize, String, String)>, // start offset, end offset, name, descriptor
}

fn u2(bytes: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]) as usize)
}

fn u4(bytes: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_be_bytes([
        *bytes.get(at)?,
        *bytes.get(at + 1)?,
        *bytes.get(at + 2)?,
        *bytes.get(at + 3)?,
    ]) as usize)
}

impl ConstantPool {
    fn parse(bytes: &[u8]) -> Option<ConstantPool> {
        if u4(bytes, 0)? != 0xCAFE_BABE {
            return None;
        }
        let count = u2(bytes, 8)?;
        let mut at = 10usize;
        let mut entries = vec![(0u8, 0usize, 0usize); count];
        let mut utf8 = BTreeMap::new();
        let mut classes = BTreeMap::new();
        let mut name_and_types = BTreeMap::new();
        let mut field_refs = BTreeMap::new();
        let mut index = 1usize;
        while index < count {
            let tag = *bytes.get(at)?;
            at += 1;
            let start = at;
            match tag {
                1 => {
                    let length = u2(bytes, at)?;
                    at += 2;
                    utf8.insert(
                        index,
                        String::from_utf8_lossy(bytes.get(at..at + length)?).into_owned(),
                    );
                    at += length;
                }
                3 | 4 => at += 4,
                5 | 6 => {
                    at += 8;
                    index += 1; // long/double take two slots
                }
                7 => {
                    classes.insert(index, u2(bytes, at)?);
                    at += 2;
                }
                8 | 16 | 19 | 20 => at += 2,
                9 => {
                    field_refs.insert(index, (u2(bytes, at)?, u2(bytes, at + 2)?));
                    at += 4;
                }
                10 | 11 | 12 | 17 | 18 => {
                    if tag == 12 {
                        name_and_types.insert(index, (u2(bytes, at)?, u2(bytes, at + 2)?));
                    }
                    at += 4;
                }
                15 => at += 3,
                _ => return None,
            }
            entries[index] = (tag, start, at);
            index += 1;
        }
        // Fields then methods.
        at += 2 + 2 + 2; // access, this, super
        let interfaces = u2(bytes, at)?;
        at += 2 + 2 * interfaces;
        at = skip_members(bytes, at, &mut None)?;
        let method_count = u2(bytes, at)?;
        at += 2;
        let mut methods = Vec::new();
        for _ in 0..method_count {
            let start = at;
            let name_index = u2(bytes, at + 2)?;
            let descriptor_index = u2(bytes, at + 4)?;
            let name = utf8.get(&name_index)?.clone();
            let descriptor = utf8.get(&descriptor_index)?.clone();
            at = skip_member(bytes, at)?;
            methods.push((start, at, name, descriptor));
        }
        Some(ConstantPool {
            entries,
            utf8,
            classes,
            name_and_types,
            field_refs,
            methods,
        })
    }

    fn utf8_at(&self, index: usize) -> Option<&str> {
        self.utf8.get(&index).map(String::as_str)
    }

    fn class_name(&self, index: usize) -> Option<&str> {
        self.utf8_at(*self.classes.get(&index)?)
    }

    fn find_method(&self, name: &str, descriptor: &str) -> Option<MemberSpan> {
        self.methods
            .iter()
            .find(|(_, _, method_name, method_descriptor)| {
                method_name == name && method_descriptor == descriptor
            })
            .map(|(start, _end, _, _)| MemberSpan {
                attributes_start: *start + 6,
                attributes_end: *start,
            })
    }

    fn field_index(&self, owner: &str, name: &str) -> Option<usize> {
        for (index, (class_index, nat_index)) in &self.field_refs {
            if self.class_name(*class_index)? != owner {
                continue;
            }
            let (name_index, _) = *self.name_and_types.get(nat_index)?;
            if self.utf8_at(name_index)? == name {
                return Some(*index);
            }
        }
        None
    }

    fn find_code_attribute(&self, bytes: &[u8], member: &MemberSpan) -> Option<CodeAttribute> {
        let mut at = member.attributes_start;
        let count = u2(bytes, at)?;
        at += 2;
        for _ in 0..count {
            let name_index = u2(bytes, at)?;
            let length = u4(bytes, at + 2)?;
            let name = self.utf8_at(name_index)?;
            if name == "Code" {
                return Some(CodeAttribute {
                    length_offset: at + 2,
                    body_start: at + 6,
                    body_end: at + 6 + length,
                });
            }
            at += 6 + length;
        }
        None
    }
}

/// `Block.column(width, minY, maxY).min(Axis.X)`, where the shape is a static
/// field built in the class initialiser from literal doubles.
fn double_constant(constants: &[Constant], index: usize) -> Option<f64> {
    match constants.get(index) {
        Some(Constant::Double(value)) => Some(*value),
        _ => match constants.get(index + 1) {
            Some(Constant::Double(value)) => Some(*value),
            _ => None,
        },
    }
}

fn column_min_x(classes: &BTreeMap<String, Vec<u8>>, owner: &str, field: &str) -> Option<f64> {
    let bytes = classes.get(owner)?;
    let class = classfile::parse(bytes).ok()?;
    let code = class
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())?;
    let mut doubles: Vec<f64> = Vec::new();
    let mut pending: Option<f64> = None;
    for instruction in vm::decode_code(&code.bytes) {
        match instruction.opcode {
            0x0e => doubles.push(0.0),
            0x0f => doubles.push(1.0),
            0x14 => match double_constant(&class.constants, instruction.index) {
                Some(value) => doubles.push(value),
                None => doubles.clear(),
            },
            0x12 | 0x13 => match class.constants.get(instruction.index) {
                Some(Constant::Float(value)) => doubles.push(*value as f64),
                _ => doubles.clear(),
            },
            0xb8 => {
                if let Some((called_owner, called_name, descriptor)) =
                    classfile::method_ref(&class.constants, instruction.index)
                {
                    if called_owner == "net/minecraft/world/level/block/Block"
                        && called_name == "column"
                    {
                        // 3-arg column(width,minY,maxY) or
                        // 4-arg column(widthX,widthZ,minY,maxY).
                        let width = if descriptor.starts_with("(DDDD)") {
                            doubles.get(doubles.len().checked_sub(4)?).copied()
                        } else {
                            doubles.get(doubles.len().checked_sub(3)?).copied()
                        };
                        pending = width;
                    }
                }
            }
            0xb3 if pending.is_some() => {
                if let Some((put_owner, put_name, _)) =
                    classfile::field_ref(&class.constants, instruction.index)
                {
                    if put_owner == owner && put_name == field {
                        // `Block.column` puts the box at
                        // `(8 - width/2) .. (8 + width/2)` pixels.
                        return pending.map(|width| (8.0 - width / 2.0) / 16.0);
                    }
                }
                pending = None;
            }
            _ => {}
        }
    }
    None
}

fn static_float(classes: &BTreeMap<String, Vec<u8>>, owner: &str, field: &str) -> Option<f64> {
    let bytes = classes.get(owner)?;
    let class = classfile::parse(bytes).ok()?;
    let code = class
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())?;
    let mut last: Option<f64> = None;
    let mut ops: Vec<(u8, usize)> = Vec::new();
    for instruction in vm::decode_code(&code.bytes) {
        ops.push((instruction.opcode, instruction.index));
        if ops.len() > 8 {
            ops.remove(0);
        }
        match instruction.opcode {
            0x0b => last = Some(0.0),
            0x0c => last = Some(1.0),
            0x0d => last = Some(2.0),
            0x14 => last = double_constant(&class.constants, instruction.index),
            0x12 | 0x13 => match class.constants.get(instruction.index) {
                Some(Constant::Float(value)) => last = Some(*value as f64),
                Some(Constant::Double(value)) => last = Some(*value),
                _ => last = None,
            },
            0xb3 => {
                if let Some((put_owner, put_name, _)) =
                    classfile::field_ref(&class.constants, instruction.index)
                {
                    if put_owner == owner && put_name == field {
                        if last.is_some() {
                            return last;
                        }
                        // `putstatic MAX_HORIZONTAL_OFFSET` where the value is
                        // `SHAPE.min(Axis.X)`: follow the shape field.
                        if let Some((_, min_index)) =
                            ops.iter().rev().find(|(opcode, _)| *opcode == 0xb6)
                        {
                            if classfile::method_ref(&class.constants, *min_index)
                                .is_some_and(|(_, name, _)| name == "min")
                            {
                                let shape = ops
                                    .iter()
                                    .rev()
                                    .filter(|(opcode, _)| *opcode == 0xb2)
                                    .find_map(|(_, index)| {
                                        let (shape_owner, shape_field, _) =
                                            classfile::field_ref(&class.constants, *index)?;
                                        if shape_owner.ends_with("Direction$Axis") {
                                            None
                                        } else {
                                            Some((shape_owner, shape_field))
                                        }
                                    })?;
                                return column_min_x(classes, &shape.0, &shape.1);
                            }
                        }
                        return None;
                    }
                }
                last = None;
            }
            _ => {
                if !matches!(instruction.opcode, 0x00..=0x0a | 0x0e..=0x10) {
                    last = None;
                }
            }
        }
    }
    None
}

fn skip_members(bytes: &[u8], mut at: usize, _out: &mut Option<()>) -> Option<usize> {
    let count = u2(bytes, at)?;
    at += 2;
    for _ in 0..count {
        at = skip_member(bytes, at)?;
    }
    Some(at)
}

fn skip_member(bytes: &[u8], at: usize) -> Option<usize> {
    let mut at = at + 6; // access, name, descriptor
    let attributes = u2(bytes, at)?;
    at += 2;
    for _ in 0..attributes {
        let length = u4(bytes, at + 2)?;
        at += 6 + length;
    }
    Some(at)
}
