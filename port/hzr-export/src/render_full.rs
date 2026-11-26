//! Rebuild `render-definitions.json` and `mob-data.json#states` from the
//! player's own `client.jar` (no `ship.hzr3` values).
//!
//! `states::state_table` gives the block-state registry (block order, property
//! value orders, state counts and default-state index), so the state-id half of
//! both tables is available. This module adds the parts that are facts about each
//! block class / each state:
//!
//! * `render-definitions.json` (`VanillaFaceVisibilityOracle`) — **complete and
//!   exact**: per block the skip-rendering rule (the first class on the chain
//!   that declares `skipRendering`), `can_occlude` (from the block's
//!   `BlockBehaviour.Properties`, including the copper collections), the
//!   `minecraft:bars` tag membership, the default state id and the block's
//!   state-id range.
//! * `mob-data.json#states` (`VanillaMobDataExporter.states`) — **blocked**, see
//!   [`blocked_tables`] and [`STATES_BLOCKED`]: a row is a path type plus eleven
//!   per-state predicates and the collision height, and six of those need the
//!   running block classes' geometry (the same ceiling `block.rs`/`shape.rs`
//!   document for `block-behaviours.json`, `cached-collision.nbt`,
//!   `occlusion.nbt` and `cached-support.nbt`).
//!
//! The module is included by path from `tests/render_full.rs`, which supplies
//! `crate::classfile` (the reader shared with `hzr-bytecode`).

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::{Map, Value};

use crate::classfile;

#[path = "../../hzr-bytecode/src/interp.rs"]
#[allow(dead_code)]
mod props;

#[path = "block.rs"]
#[allow(dead_code)]
mod block;

#[path = "states.rs"]
#[allow(dead_code)]
mod states;

/// The tables/sections that stay out of reach from the jar alone, and why.
pub fn blocked_tables() -> &'static [(&'static str, &'static str)] {
    &[(
        "mob-data.json#states",
        "one row per block state = WalkNodeEvaluator path type + 11 state predicates + \
         collision height; six of those are block geometry and the rest are block code \
         (see STATES_BLOCKED)",
    )]
}

/// `VanillaMobDataExporter.states()` row columns that cannot be rebuilt from the
/// jar's data, and the code that computes each one.
///
/// A row is `[pathType, flags, height]`; the flags are
/// `1 LAND|2 WATER|4 AIR|8 zombie|16 ocelot|32 polar bear|64 redstone| \
///  128 empty-spawn|256 face-sturdy-up|512 solid|1024 solid-render`.
pub const STATES_BLOCKED: [(&str, &str); 9] = [
    (
        "path type",
        "WalkNodeEvaluator.getPathTypeFromState: an instanceof/`state.is(tag|block)`/\
         state-value ladder over 35,723 states producing 17 outcomes",
    ),
    (
        "flags 1/2/4 (pathfindable LAND/WATER/AIR)",
        "BlockBehaviour.isPathfindable overrides on the block classes; the default is \
         not the only answer (doors, trapdoors, leaves, fences, ... specialise it)",
    ),
    (
        "flags 8/16/32 (isValidSpawn zombie/ocelot/polar bear)",
        "Properties.isValidSpawn predicate lambdas built in Blocks.<clinit> plus block \
         overrides; needs the running predicate",
    ),
    (
        "flag 64 (isRedstoneConductor)",
        "Properties.isRedstoneConductor predicate lambdas built in Blocks.<clinit>",
    ),
    (
        "flag 128 (NaturalSpawner.isValidEmptySpawnBlock)",
        "state.isCollisionShapeFullBlock(level,pos) plus isSignalSource, fluid emptiness, \
         the PREVENT_MOB_SPAWNING_INSIDE tag and EntityType.isBlockDangerous",
    ),
    (
        "flag 256 (isFaceSturdy UP)",
        "the state's support/face shape (SupportType.FULL), i.e. block geometry",
    ),
    (
        "flag 512 (isSolid / legacySolid)",
        "BlockStateBase.calculateSolid: forceSolidOn, forceSolidOff, then \
         `hasCollision`/collision-shape-full-block; the last two need the shape",
    ),
    (
        "flag 1024 (isSolidRender)",
        "Block.isShapeFullBlock(occlusionShape), i.e. the state's occlusion geometry",
    ),
    (
        "height (collision shape max Y, 0 when empty)",
        "state.getCollisionShape(empty, ZERO).max(Axis.Y), i.e. the same collision \
         geometry `cached-collision.nbt` needs",
    ),
];

/// The number of states `mob-data.json#states` must have one row for. The
/// registry itself (ids, order, property values) is available from
/// `states::state_table`; the predicates above are the blocker.
pub fn states_row_count(jar_path: &Path) -> Result<usize, String> {
    Ok(states::state_table(jar_path)?.state_count())
}

/// What the probe found, for the tests to report.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Probe {
    pub blocks: usize,
    pub unnamed: usize,
    pub registry: usize,
    pub class_conflicts: Vec<String>,
    pub unmodelled_sinks: Vec<String>,
    /// Blocks whose value was taken from another block of the same class
    /// because the evaluator could not reach them (`id (class)`).
    pub inherited: Vec<String>,
    /// What each fill-in pass did, for the report.
    pub notes: Vec<String>,
}

// ---------------------------------------------------------------------------
// jar access
// ---------------------------------------------------------------------------

struct JarFiles {
    files: BTreeMap<String, Vec<u8>>,
}

impl JarFiles {
    fn open(path: &Path) -> Result<Self, String> {
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
            use std::io::Read;
            entry
                .read_to_end(&mut data)
                .map_err(|error| format!("{name}: {error}"))?;
            files.insert(name, data);
        }
        Ok(Self { files })
    }

    fn class(&self, internal: &str) -> Option<classfile::ClassFile> {
        let bytes = self.files.get(&format!("{internal}.class"))?;
        classfile::parse(bytes).ok()
    }

    fn text(&self, name: &str) -> Option<&[u8]> {
        self.files.get(name).map(Vec::as_slice)
    }
}

impl props::Loader for JarFiles {
    fn load(&mut self, name: &str) -> Option<classfile::ClassFile> {
        self.class(name)
    }
}

// ---------------------------------------------------------------------------
// block properties (can_occlude)
// ---------------------------------------------------------------------------

/// Block id (without `minecraft:`) -> whether its `Properties.canOcclude` is set.
///
/// `Properties.canOcclude` starts `true`; vanilla clears it from exactly two
/// methods, `noCollision()` (which the interpreter models as `no_collision`) and
/// `noOcclusion()` (which it records as an unmodelled chain call). Both
/// `ofLegacyCopy`/`ofFullCopy` copy the whole property struct in the
/// interpreter, so inheritance behaves like vanilla.
pub fn can_occlude(jar_path: &Path) -> Result<BTreeMap<String, bool>, String> {
    can_occlude_probed(jar_path).map(|(table, _)| table)
}

/// Diagnostics for the property evaluation (block/registration counts, class
/// consistency of `can_occlude`, property calls the interpreter did not model).
pub fn probe(jar_path: &Path) -> Result<Probe, String> {
    can_occlude_probed(jar_path).map(|(_, probe)| probe)
}

/// The block names `Blocks.<clinit>` gave the property evaluator, in
/// registration order (empty when the registration's id was not resolved).
pub fn registration_names(jar_path: &Path) -> Result<Vec<String>, String> {
    let files = JarFiles::open(jar_path)?;
    let ids = files
        .class("net/minecraft/references/BlockItemIds")
        .map(|class| props::ids_of(&class))
        .ok_or_else(|| "BlockItemIds is not in the jar".to_string())?;
    let mut jar = files;
    Ok(props::blocks_with(&mut jar, &ids)
        .into_iter()
        .map(|block| block.name)
        .collect())
}

fn can_occlude_probed(jar_path: &Path) -> Result<(BTreeMap<String, bool>, Probe), String> {
    let registry = block::registry(jar_path)?;
    let files = JarFiles::open(jar_path)?;
    let ids = files
        .class("net/minecraft/references/BlockItemIds")
        .map(|class| props::ids_of(&class))
        .ok_or_else(|| "BlockItemIds is not in the jar".to_string())?;
    let mut jar = files;
    let blocks = props::blocks_with(&mut jar, &ids);
    let blocks_class = jar
        .class("net/minecraft/world/level/block/Blocks")
        .ok_or_else(|| "Blocks is not in the jar".to_string())?;
    let mut probe = Probe {
        blocks: blocks.len(),
        unnamed: blocks.iter().filter(|block| block.name.is_empty()).count(),
        registry: registry.len(),
        ..Probe::default()
    };
    probe.unmodelled_sinks = properties_unmodelled(&blocks);
    // The check below treats `noOcclusion` as an unmodelled property call, which
    // is how `hzr-bytecode`'s evaluator records it. If that evaluator starts
    // modelling the method as a field, this derivation must be updated rather
    // than silently answering `true` for every block.
    let no_occlusion_blocks = blocks
        .iter()
        .filter(|block| {
            block
                .properties
                .unmodelled
                .iter()
                .any(|name| name == "noOcclusion")
        })
        .count();
    if no_occlusion_blocks == 0 {
        return Err(
            "hzr-bytecode's Properties evaluator no longer records noOcclusion as an unmodelled \
             call; update can_occlude"
                .to_string(),
        );
    }
    probe
        .notes
        .push(format!("direct noOcclusion calls: {no_occlusion_blocks}"));

    // The properties evaluator cannot name every registration: blocks created
    // inside `ColorCollection`/`WeatheringCopperCollection` loops are pushed in
    // registration order but without an id. `block::registry` has the ids and
    // classes in the same order (verified 1,286/1,286), so the two lists are
    // aligned through the named anchors: between two anchors the unnamed
    // registrations correspond one-to-one, in order, to the registry's ids.
    let registry_index: BTreeMap<&str, usize> = registry
        .iter()
        .enumerate()
        .map(|(index, block)| (block.id.trim_start_matches("minecraft:"), index))
        .collect();
    let mut anchors: Vec<(usize, usize)> = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        if block.name.is_empty() {
            continue;
        }
        if let Some(registry_position) = registry_index.get(block.name.as_str()) {
            anchors.push((index, *registry_position));
        }
    }
    let monotonic = anchors.windows(2).all(|pair| pair[0].1 < pair[1].1);
    if !monotonic {
        return Err("block registration anchors are not in registry order".to_string());
    }
    let mut mapped: Vec<Option<usize>> = vec![None; blocks.len()];
    let mut segment_start = (0usize, 0usize);
    for anchor in anchors
        .iter()
        .copied()
        .chain(std::iter::once((blocks.len(), registry.len())))
    {
        let (block_from, registry_from) = segment_start;
        let (block_to, registry_to) = anchor;
        if block_to - block_from == registry_to - registry_from {
            for offset in 0..(block_to - block_from) {
                mapped[block_from + offset] = Some(registry_from + offset);
            }
        } else {
            probe.notes.push(format!(
                "unaligned segment: {block_from}..{block_to} blocks vs {registry_from}..{registry_to} registry"
            ));
        }
        if anchor.0 < blocks.len() {
            mapped[anchor.0] = Some(anchor.1);
        }
        segment_start = (anchor.0 + 1, anchor.1 + 1);
    }

    let mut values: Vec<Option<bool>> = vec![None; registry.len()];
    let mut blocked_directly: Vec<bool> = vec![false; registry.len()];
    for (index, properties) in blocks.iter().enumerate() {
        let Some(position) = mapped[index] else {
            continue;
        };
        let no_occlusion = properties
            .properties
            .unmodelled
            .iter()
            .any(|name| name == "noOcclusion");
        let value = !(properties.properties.no_collision || no_occlusion);
        blocked_directly[position] = !value;
        values[position] = Some(value);
    }

    // `WeatheringCopperCollection.registerBlocks` composes `ByState` records and
    // lambdas; the properties evaluator does not execute it, so its 14 families
    // are the unmapped rows. Each call site names one straight-line
    // `Function<WeatherState, Properties>`; `noOcclusion`/`noCollision` in that
    // body is the whole answer, and the same function builds weathering and
    // waxed halves, so its eight blocks share the value.
    let copper = copper_families(&jar, &blocks_class)?;
    let unmapped: Vec<usize> = values
        .iter()
        .enumerate()
        .filter(|(_, value)| value.is_none())
        .map(|(index, _)| index)
        .collect();
    if !copper.is_empty() && unmapped.len() == copper.len() * 8 {
        for (site, value) in copper.iter().enumerate() {
            for offset in 0..8 {
                values[unmapped[site * 8 + offset]] = Some(*value);
            }
        }
        probe
            .notes
            .push(format!("copper: {} families x 8", copper.len()));
    } else if !copper.is_empty() {
        probe.notes.push(format!(
            "copper: {} families do not fit {} unmapped rows",
            copper.len(),
            unmapped.len()
        ));
    }

    // Direct `ofLegacyCopy`/`ofFullCopy` registrations: the evaluator cannot
    // follow a `getstatic Blocks.X` argument (it resolves the field as an enum
    // token), so those blocks fell back to a fresh `Properties`. Vanilla copies
    // the source's `canOcclude`, so the source block's value is the answer.
    for (target_field, source_field) in direct_copies(&jar, &blocks_class)? {
        let Some(&target) = registry_index.get(lower(&target_field).as_str()) else {
            probe
                .notes
                .push(format!("copy target {target_field} not in registry"));
            continue;
        };
        if blocked_directly[target] {
            continue;
        }
        let Some(&source) = registry_index.get(lower(&source_field).as_str()) else {
            probe
                .notes
                .push(format!("copy source {source_field} not in registry"));
            continue;
        };
        if let Some(value) = values[source] {
            values[target] = Some(value);
        }
    }

    // Anything still unreachable inherits its block class's value: vanilla builds
    // one class's properties through the same helpers, so `canOcclude` is a class
    // constant.
    let mut seen_classes: BTreeMap<String, bool> = BTreeMap::new();
    for (position, value) in values.iter().enumerate() {
        if let Some(value) = value {
            seen_classes
                .entry(registry[position].class.clone())
                .or_insert(*value);
        }
    }
    for position in 0..registry.len() {
        if values[position].is_some() {
            continue;
        }
        if let Some(value) = seen_classes.get(&registry[position].class).copied() {
            values[position] = Some(value);
            probe.inherited.push(format!(
                "{} ({})",
                registry[position].id, registry[position].class
            ));
        }
    }

    for (position, value) in values.iter().enumerate() {
        if let (Some(value), Some(expected)) = (value, seen_classes.get(&registry[position].class))
        {
            if value != expected {
                probe.class_conflicts.push(format!(
                    "{}: {value} vs {} ({})",
                    registry[position].class, expected, registry[position].id
                ));
            }
        }
    }

    let mut out = BTreeMap::new();
    for (position, value) in values.iter().enumerate() {
        if let Some(value) = value {
            out.insert(
                registry[position]
                    .id
                    .trim_start_matches("minecraft:")
                    .to_string(),
                *value,
            );
        }
    }
    Ok((out, probe))
}

fn lower(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// The `Properties`-building `Function<WeatherState, Properties>` of every
/// `WeatheringCopperCollection.registerBlocks` call, in call order: `true` when
/// the body never calls `noOcclusion`/`noCollision`.
fn copper_families(
    files: &JarFiles,
    blocks_class: &classfile::ClassFile,
) -> Result<Vec<bool>, String> {
    let code = blocks_class
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
        .map(|code| code.bytes.clone())
        .ok_or_else(|| "Blocks.<clinit> has no code".to_string())?;
    let mut sites = Vec::new();
    let mut indys: Vec<usize> = Vec::new();
    for instruction in props::decode(&code) {
        match instruction.opcode {
            0xba => indys.push(instruction.index.unwrap_or(0)),
            0xb8 => {
                let Some((owner, name, _)) =
                    classfile::method_ref(&blocks_class.constants, instruction.index.unwrap_or(0))
                else {
                    continue;
                };
                if owner.ends_with("WeatheringCopperCollection") && name == "registerBlocks" {
                    if let Some(&properties_function) = indys.last() {
                        match indy_target(blocks_class, properties_function) {
                            Some((owner, method)) => {
                                sites.push(copper_function_value(files, &owner, &method)?)
                            }
                            None => {
                                return Err(format!(
                                    "copper registerBlocks at index {properties_function}: no bootstrap target ({} indy args)",
                                    indys.len()
                                ))
                            }
                        }
                    }
                }
                indys.clear();
            }
            0xb6 | 0xb7 | 0xb9 | 0xb2 | 0xb3 | 0xb4 | 0xb5 => indys.clear(),
            _ => {}
        }
    }
    Ok(sites)
}

/// `true` when `method`'s body has no `Properties.noOcclusion`/`noCollision`.
fn copper_function_value(files: &JarFiles, owner: &str, method: &str) -> Result<bool, String> {
    let class = files
        .class(owner)
        .ok_or_else(|| format!("{owner} is not in the jar"))?;
    let body = class
        .method(method)
        .and_then(|method| method.code.as_ref())
        .map(|code| code.bytes.clone())
        .ok_or_else(|| format!("{owner}.{method} has no code"))?;
    for instruction in props::decode(&body) {
        if instruction.opcode != 0xb6 {
            continue;
        }
        let Some((owner, name, _)) =
            classfile::method_ref(&class.constants, instruction.index.unwrap_or(0))
        else {
            continue;
        };
        if owner.ends_with("Properties") && (name == "noOcclusion" || name == "noCollision") {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Direct registrations whose properties start from
/// `Properties.ofLegacyCopy`/`ofFullCopy(Blocks.X)`: `(target field, source field)`.
fn direct_copies(
    files: &JarFiles,
    blocks_class: &classfile::ClassFile,
) -> Result<Vec<(String, String)>, String> {
    let code = blocks_class
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
        .map(|code| code.bytes.clone())
        .ok_or_else(|| "Blocks.<clinit> has no code".to_string())?;
    let mut out = Vec::new();
    let mut last_block: Option<String> = None;
    let mut copy_source: Option<String> = None;
    let mut registration_source: Option<Option<String>> = None;
    for instruction in props::decode(&code) {
        let reference = instruction.index.unwrap_or(0);
        match instruction.opcode {
            0xb2 => {
                if let Some((owner, field, descriptor)) =
                    classfile::field_ref(&blocks_class.constants, reference)
                {
                    if owner.ends_with("Blocks")
                        && descriptor.starts_with("Lnet/minecraft/world/level/block/Block;")
                    {
                        last_block = Some(field);
                    }
                }
            }
            0xb8 => {
                if let Some((owner, name, _)) =
                    classfile::method_ref(&blocks_class.constants, reference)
                {
                    if owner.ends_with("Properties")
                        && (name == "ofLegacyCopy" || name == "ofFullCopy")
                    {
                        copy_source = last_block.take();
                    } else if owner.ends_with("Blocks") && name == "register" {
                        registration_source = Some(copy_source.take());
                    }
                }
            }
            0xb3 => {
                if let Some((owner, field, descriptor)) =
                    classfile::field_ref(&blocks_class.constants, reference)
                {
                    if owner.ends_with("Blocks")
                        && descriptor.starts_with("Lnet/minecraft/world/level/block/Block;")
                    {
                        if let Some(Some(source)) = registration_source.take() {
                            out.push((field, source));
                        } else {
                            registration_source = None;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let _ = files;
    Ok(out)
}

/// The implementation method an `invokedynamic` call site's bootstrap names.
pub fn indy_target(class: &classfile::ClassFile, index: usize) -> Option<(String, String)> {
    let classfile::Constant::InvokeDynamic { bootstrap, .. } = class.constants.get(index)? else {
        return None;
    };
    let entry = class.bootstraps.get(*bootstrap)?;
    for argument in &entry.arguments {
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
        let name_index: usize = name.trim_start_matches('#').parse().ok()?;
        let owner_name = class_constant_name(&class.constants, owner_index)?;
        let method_name = match class.constants.get(name_index)? {
            classfile::Constant::NameAndType { name, kind } => {
                let name_at: usize = name.trim_start_matches('#').parse().ok()?;
                let kind_at: usize = kind.trim_start_matches('#').parse().ok()?;
                let _descriptor = classfile::utf8(&class.constants, kind_at);
                classfile::utf8(&class.constants, name_at)
            }
            classfile::Constant::Utf8(text) => text.clone(),
            _ => continue,
        };
        return Some((owner_name, method_name));
    }
    None
}

/// A `Class` constant's internal name (or a `Utf8` constant's text).
fn class_constant_name(constants: &[classfile::Constant], index: usize) -> Option<String> {
    match constants.get(index)? {
        classfile::Constant::Class(reference) => {
            let inner: usize = reference.trim_start_matches('#').parse().ok()?;
            Some(classfile::utf8(constants, inner))
        }
        classfile::Constant::Utf8(text) => Some(text.clone()),
        _ => None,
    }
}

fn properties_unmodelled(blocks: &[props::Block]) -> Vec<String> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for block in blocks {
        for name in &block.properties.unmodelled {
            *counts.entry(name.as_str()).or_default() += 1;
        }
    }
    let mut out: Vec<(&str, usize)> = counts.into_iter().collect();
    out.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    out.into_iter()
        .take(20)
        .map(|(name, count)| format!("{name}: {count}"))
        .collect()
}

/// The compact JSON text Gson writes for `render-definitions.json` is keyed by
/// block id in registry order; `Value` maps sort their keys, so the table is
/// exposed as ordered entries for byte comparison and as a `Value` for semantic
/// comparison.
pub fn can_occlude_value(jar_path: &Path) -> Result<Value, String> {
    let table = can_occlude(jar_path)?;
    let mut object = Map::new();
    for (name, value) in table {
        object.insert(format!("minecraft:{name}"), Value::Bool(value));
    }
    Ok(Value::Object(object))
}

// ---------------------------------------------------------------------------
// render-definitions.json
// ---------------------------------------------------------------------------

const BLOCK_PREFIX: &str = "net/minecraft/world/level/block/";
/// `Block.skipRendering(BlockState, BlockState, Direction)`, the method
/// `VanillaFaceVisibilityOracle.rule` looks up on each class of the chain.
const SKIP_RENDERING: &str = "(Lnet/minecraft/world/level/block/state/BlockState;Lnet/minecraft/world/level/block/state/BlockState;Lnet/minecraft/core/Direction;)Z";

/// One row of `render-definitions.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub rule: &'static str,
    pub can_occlude: bool,
    pub bars_tag: bool,
    pub default: u32,
    pub states: Vec<u32>,
}

/// The table in registry order, plus cross-check notes for the report.
#[derive(Debug, Clone, Default)]
pub struct RenderDefinitions {
    pub entries: Vec<(String, Definition)>,
    pub notes: Vec<String>,
}

/// `render-definitions.json` from the jar alone.
pub fn render_definitions(jar_path: &Path) -> Result<RenderDefinitions, String> {
    let files = JarFiles::open(jar_path)?;
    let registry = block::registry(jar_path)?;
    let state_table = states::state_table(jar_path)?;
    let can_occlude = can_occlude(jar_path)?;
    let index = class_index(&files);
    let bars = bars_members(&files)?;
    let mut class_cache: BTreeMap<String, Option<classfile::ClassFile>> = BTreeMap::new();
    if state_table.blocks.len() != registry.len() {
        return Err(format!(
            "state table has {} blocks, block registry {}",
            state_table.blocks.len(),
            registry.len()
        ));
    }
    let mut entries = Vec::with_capacity(registry.len());
    let mut next_state = 0u32;
    let mut notes = Vec::new();
    for (block, states) in registry.iter().zip(state_table.blocks.iter()) {
        if block.id != states.id {
            return Err(format!(
                "state table/block registry disagree: {} vs {}",
                states.id, block.id
            ));
        }
        let rule = skip_rendering_rule(&files, &index, &block.chain, &mut class_cache)?;
        let name = block.id.trim_start_matches("minecraft:");
        let occlude = *can_occlude
            .get(name)
            .ok_or_else(|| format!("{}: no can_occlude", block.id))?;
        let bars_tag = bars.contains(block.id.as_str());
        let count = states.state_count as u32;
        let default = next_state + states.default_index as u32;
        let state_ids: Vec<u32> = (next_state..next_state + count).collect();
        next_state += count;
        entries.push((
            block.id.clone(),
            Definition {
                rule,
                can_occlude: occlude,
                bars_tag,
                default,
                states: state_ids,
            },
        ));
    }
    if next_state as usize != state_table.state_count() {
        notes.push(format!(
            "state ids ended at {next_state}, table says {}",
            state_table.state_count()
        ));
    }
    Ok(RenderDefinitions { entries, notes })
}

/// The table as a JSON object (keys sorted; registry order is in [`entries`]).
pub fn render_definitions_value(jar_path: &Path) -> Result<Value, String> {
    let table = render_definitions(jar_path)?;
    let mut object = Map::new();
    for (id, definition) in &table.entries {
        let mut row = Map::new();
        row.insert(
            "default".into(),
            Value::Number(serde_json::Number::from(definition.default)),
        );
        row.insert("can_occlude".into(), Value::Bool(definition.can_occlude));
        row.insert("bars_tag".into(), Value::Bool(definition.bars_tag));
        row.insert("rule".into(), Value::String(definition.rule.to_string()));
        row.insert(
            "states".into(),
            Value::Array(
                definition
                    .states
                    .iter()
                    .map(|id| Value::Number(serde_json::Number::from(*id)))
                    .collect(),
            ),
        );
        object.insert(id.clone(), Value::Object(row));
    }
    Ok(Value::Object(object))
}

/// Write `render-definitions.json` the way `VanillaFaceVisibilityOracle` does:
/// a `TreeMap` keyed by block id (sorted), five fields in `Map.of` order, compact
/// JSON. This is byte-identical to the Java file when the jar matches.
pub fn write_render_definitions(
    jar_path: &Path,
    out: impl AsRef<Path>,
) -> Result<RenderDefinitions, String> {
    let table = render_definitions(jar_path)?;
    let mut sorted: Vec<&(String, Definition)> = table.entries.iter().collect();
    sorted.sort_by(|left, right| left.0.cmp(&right.0));
    let mut text = String::from("{");
    for (slot, (id, definition)) in sorted.iter().enumerate() {
        if slot > 0 {
            text.push(',');
        }
        let key = serde_json::to_string(id).map_err(|error| error.to_string())?;
        text.push_str(&key);
        text.push_str(&format!(
            ":{{\"default\":{},\"can_occlude\":{},\"bars_tag\":{},\"rule\":\"{}\",\"states\":[",
            definition.default, definition.can_occlude, definition.bars_tag, definition.rule
        ));
        for (index, state) in definition.states.iter().enumerate() {
            if index > 0 {
                text.push(',');
            }
            text.push_str(&state.to_string());
        }
        text.push_str("]}");
    }
    text.push('}');
    let out = out.as_ref();
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    std::fs::write(out, text).map_err(|error| format!("{}: {error}", out.display()))?;
    Ok(table)
}

/// Simple name -> internal name for every class in the block package. The names
/// are unique inside it (checked: only `package-info` repeats).
fn class_index(files: &JarFiles) -> BTreeMap<String, String> {
    let mut index = BTreeMap::new();
    for name in files.files.keys() {
        let Some(internal) = name.strip_suffix(".class") else {
            continue;
        };
        if !internal.starts_with(BLOCK_PREFIX) || internal.contains('$') {
            continue;
        }
        let simple = internal.rsplit('/').next().unwrap_or(internal);
        if simple == "package-info" {
            continue;
        }
        index.insert(simple.to_string(), internal.to_string());
    }
    index
}

/// The rule `VanillaFaceVisibilityOracle` writes: walk the block's superclass
/// chain to the first class that declares `skipRendering`, then switch on that
/// class's simple name.
fn skip_rendering_rule(
    files: &JarFiles,
    index: &BTreeMap<String, String>,
    chain: &[String],
    cache: &mut BTreeMap<String, Option<classfile::ClassFile>>,
) -> Result<&'static str, String> {
    for simple in chain {
        let Some(internal) = index.get(simple) else {
            continue;
        };
        let class = if let Some(hit) = cache.get(internal) {
            hit.clone()
        } else {
            let parsed = files.class(internal);
            cache.insert(internal.clone(), parsed.clone());
            parsed
        };
        let Some(class) = class else { continue };
        let declares = class
            .methods
            .iter()
            .any(|method| method.name == "skipRendering" && method.descriptor == SKIP_RENDERING);
        if !declares {
            continue;
        }
        return match simple.as_str() {
            "BlockBehaviour" => Ok("never"),
            "HalfTransparentBlock" | "PowderSnowBlock" => Ok("same_block"),
            "IronBarsBlock" => Ok("bars"),
            "LeavesBlock" => Ok("leaves"),
            "LiquidBlock" => Ok("liquid"),
            "MangroveRootsBlock" => Ok("mangrove_roots"),
            other => Err(format!("unhandled skipRendering owner {other}")),
        };
    }
    // BlockBehaviour itself declares `skipRendering`; it is not in the chain
    // `block::registry` exposes, so its rule is the fallback.
    Ok("never")
}

/// `minecraft:bars` tag members from the jar's data pack.
fn bars_members(files: &JarFiles) -> Result<std::collections::BTreeSet<String>, String> {
    let data = files
        .text("data/minecraft/tags/block/bars.json")
        .ok_or_else(|| "data/minecraft/tags/block/bars.json is not in the jar".to_string())?;
    let value: Value =
        serde_json::from_slice(data).map_err(|error| format!("bars tag: {error}"))?;
    let mut members = std::collections::BTreeSet::new();
    if let Some(values) = value.get("values").and_then(Value::as_array) {
        for entry in values {
            if let Some(id) = entry.as_str() {
                if !id.starts_with('#') {
                    members.insert(id.to_string());
                }
            }
        }
    }
    Ok(members)
}
