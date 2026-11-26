//! The `worldgen/blocks/structure-blocks.json` table: what a structure template needs
//! to know about every block state.
//!
//! The Java exporter is
//! `mods/horizon/src/test/java/dev/horizonlab/parity/VanillaStructureBlockExporter.java`:
//!
//! ```text
//! states          Block.BLOCK_STATE_REGISTRY.size()
//! rotate[i]       [Block.getId(state.rotate(CLOCKWISE_90)), ...(CLOCKWISE_180), ...(COUNTERCLOCKWISE_90)]
//! mirror[i]       [Block.getId(state.mirror(LEFT_RIGHT)), Block.getId(state.mirror(FRONT_BACK))]
//! full_block[i]   !block.hasDynamicShape() && state.isCollisionShapeFullBlock(EmptyBlockGetter, ZERO)
//! loot_containers states whose block's `newBlockEntity` is a `RandomizableContainer`
//! block_entities  block entity type id -> the states that carry it
//! ```
//!
//! # Where the answers live
//!
//! The state ids are the ones [`states`] builds from the jar: blocks in registry order,
//! each block's properties sorted by name, the last property varying fastest. Everything
//! else is block code — `BlockBehaviour.rotate`/`mirror` are overridable, and 66 block
//! classes override `rotate` while 61 override `mirror`. The families:
//!
//! * `facing` turns with the rotation (`HorizontalDirectionalBlock` and the ~45 classes
//!   that repeat its four lines), and `mirror` on those is
//!   `state.rotate(Mirror.getRotation(facing))`, i.e. a half turn for one axis.
//! * Fences, walls, vines, redstone wire, tripwire and mossy carpets permute their
//!   `north`/`east`/`south`/`west` properties (all reads see the original state).
//! * Huge mushrooms and multiface blocks permute all six direction properties.
//! * Rails and stairs switch on their shape enum; stairs' mirror half-turns the stair and
//!   swaps inner/outer left/right.
//! * Banners, standing signs and skulls turn their 16-value `rotation` integer.
//! * `RotatedPillarBlock` (and the two blocks that reuse its helper) swap `axis` X/Z for a
//!   quarter turn; the nether portal is the same on its two axes.
//! * Crafter and jigsaw blocks turn their `FrontAndTop` `orientation`.
//!
//! A block whose class is in none of these families is a hard error rather than a silent
//! identity, so an unported jar fails loudly.
//!
//! # Block entities
//!
//! `loot_containers` and `block_entities` are derived, not hand-listed: a block's block
//! entity is the `new` in the nearest `newBlockEntity` of its class chain, `BlockEntityTypes`
//! names that entity class to a type id (through `BlockEntityTypeIds`), and a block entity
//! is a `RandomizableContainer` when it or a superclass implements the interface. One type,
//! `minecraft:vault`, comes out of the jar this way and is missing from the prepared copy.
//!
//! `full_block` is the remaining gap: it needs each block's collision `VoxelShape`
//! (`getCollisionShape` is code over `Shapes.box` values, i.e. the shape port), so [`write`]
//! reports it in [`Coverage::missing`] and writes `false` for every state rather than
//! guessing.
//!
//! Nothing here reads `ship.hzr3`.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

#[path = "../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;

#[path = "block.rs"]
#[allow(dead_code)]
mod block;

#[path = "states.rs"]
#[allow(dead_code)]
mod states;

use classfile::ClassFile;

/// The file `ship.hzr3` carries, below the bundle root.
pub const TABLE: &str = "worldgen/blocks/structure-blocks.json";

/// What was produced, and what is still out of reach.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub produced: Vec<String>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
}

/// The six sections of the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub states: usize,
    /// Three rotations per state: `CLOCKWISE_90`, `CLOCKWISE_180`, `COUNTERCLOCKWISE_90`.
    pub rotate: Vec<[u32; 3]>,
    /// `LEFT_RIGHT`, `FRONT_BACK`.
    pub mirror: Vec<[u32; 2]>,
    pub full_block: Vec<bool>,
    pub loot_containers: Vec<u32>,
    /// Block entity type id -> states, in first-appearance order (Gson's insertion order).
    pub block_entities: Vec<(String, Vec<u32>)>,
}

// ---------------------------------------------------------------------------
// Directions, axes, rotations and mirrors
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Axis {
    X,
    Y,
    Z,
}

/// `net.minecraft.core.Direction`, by declaration order (which its ordinals use).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dir {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

const DIRECTIONS: [Dir; 6] = [
    Dir::Down,
    Dir::Up,
    Dir::North,
    Dir::South,
    Dir::West,
    Dir::East,
];

impl Dir {
    fn parse(name: &str) -> Option<Dir> {
        Some(match name {
            "down" => Dir::Down,
            "up" => Dir::Up,
            "north" => Dir::North,
            "south" => Dir::South,
            "west" => Dir::West,
            "east" => Dir::East,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Dir::Down => "down",
            Dir::Up => "up",
            Dir::North => "north",
            Dir::South => "south",
            Dir::West => "west",
            Dir::East => "east",
        }
    }

    fn axis(self) -> Axis {
        match self {
            Dir::Down | Dir::Up => Axis::Y,
            Dir::North | Dir::South => Axis::Z,
            Dir::West | Dir::East => Axis::X,
        }
    }

    fn opposite(self) -> Dir {
        match self {
            Dir::Down => Dir::Up,
            Dir::Up => Dir::Down,
            Dir::North => Dir::South,
            Dir::South => Dir::North,
            Dir::West => Dir::East,
            Dir::East => Dir::West,
        }
    }

    /// `Direction.getClockWise()`; the vertical directions do not move.
    fn clockwise(self) -> Dir {
        match self {
            Dir::North => Dir::East,
            Dir::East => Dir::South,
            Dir::South => Dir::West,
            Dir::West => Dir::North,
            other => other,
        }
    }

    fn counter_clockwise(self) -> Dir {
        self.clockwise().clockwise().clockwise()
    }
}

/// `net.minecraft.world.level.block.Rotation`, by ordinal (NONE, CLOCKWISE_90, ...).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rotation {
    None,
    Clockwise90,
    Clockwise180,
    CounterClockwise90,
}

/// The three rotations the export reports, in `Rotation` ordinal order.
const ROTATIONS: [Rotation; 3] = [
    Rotation::Clockwise90,
    Rotation::Clockwise180,
    Rotation::CounterClockwise90,
];

/// `net.minecraft.world.level.block.Mirror`, by ordinal (NONE, LEFT_RIGHT, FRONT_BACK).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mirror {
    LeftRight,
    FrontBack,
}

impl Rotation {
    /// `Rotation.rotate(Direction)`: a quarter turn moves the horizontal directions on;
    /// the vertical ones stay.
    fn direction(self, direction: Dir) -> Dir {
        if direction.axis() == Axis::Y {
            return direction;
        }
        match self {
            Rotation::None => direction,
            Rotation::Clockwise90 => direction.clockwise(),
            Rotation::Clockwise180 => direction.opposite(),
            Rotation::CounterClockwise90 => direction.counter_clockwise(),
        }
    }

    /// `Rotation.rotate(Direction.Axis)`: a quarter turn swaps X and Z.
    fn axis(self, axis: Axis) -> Axis {
        match self {
            Rotation::Clockwise90 | Rotation::CounterClockwise90 => match axis {
                Axis::X => Axis::Z,
                Axis::Z => Axis::X,
                Axis::Y => Axis::Y,
            },
            _ => axis,
        }
    }

    /// `Rotation.rotate(int value, int max)`: the sixteen-value `rotation` property.
    fn value(self, value: i32, max: i32) -> i32 {
        let shift = match self {
            Rotation::None => return value,
            Rotation::Clockwise90 => max / 4,
            Rotation::Clockwise180 => max / 2,
            Rotation::CounterClockwise90 => max * 3 / 4,
        };
        (value + shift).rem_euclid(max)
    }
}

impl Mirror {
    /// `Mirror.mirror(Direction)`: `LEFT_RIGHT` flips the Z axis, `FRONT_BACK` the X axis.
    fn direction(self, direction: Dir) -> Dir {
        match self {
            Mirror::LeftRight if direction.axis() == Axis::Z => direction.opposite(),
            Mirror::FrontBack if direction.axis() == Axis::X => direction.opposite(),
            _ => direction,
        }
    }

    /// `Mirror.getRotation(Direction)`.
    fn rotation(self, direction: Dir) -> Rotation {
        let flipped = match self {
            Mirror::LeftRight => direction.axis() == Axis::Z,
            Mirror::FrontBack => direction.axis() == Axis::X,
        };
        if flipped {
            Rotation::Clockwise180
        } else {
            Rotation::None
        }
    }

    /// `Mirror.mirror(int value, int max)`.
    fn value(self, value: i32, max: i32) -> i32 {
        let half = max / 2;
        let wrapped = if value > half { value - max } else { value };
        let turned = match self {
            Mirror::LeftRight => half - wrapped + max,
            Mirror::FrontBack => max - wrapped,
        };
        turned.rem_euclid(max)
    }
}

// ---------------------------------------------------------------------------
// The block state model
// ---------------------------------------------------------------------------

/// One block's place in the state table.
struct BlockState {
    id: String,
    /// Properties sorted by name — `StateDefinition`'s order, which the ids use.
    properties: Vec<(String, Vec<String>)>,
    /// `state id = start + sum(value index * multiplier)`.
    multipliers: Vec<usize>,
    start: u32,
    count: u32,
}

impl BlockState {
    fn slot(&self, name: &str) -> Option<usize> {
        self.properties
            .iter()
            .position(|(property, _)| property == name)
    }

    fn value_name(&self, slot: usize, index: u8) -> Option<&str> {
        self.properties[slot]
            .1
            .get(index as usize)
            .map(String::as_str)
    }

    fn index_of(&self, slot: usize, name: &str) -> Option<u8> {
        self.properties[slot]
            .1
            .iter()
            .position(|candidate| candidate == name)
            .map(|index| index as u8)
    }

    fn id_of(&self, values: &[u8]) -> u32 {
        let mut index = 0usize;
        for (slot, value) in values.iter().enumerate() {
            index += *value as usize * self.multipliers.get(slot).copied().unwrap_or(0);
        }
        self.start + index as u32
    }

    fn values_from(&self, id: u32) -> Vec<u8> {
        let mut rest = (id - self.start) as usize;
        let mut values = vec![0u8; self.properties.len()];
        for slot in 0..self.properties.len() {
            let multiplier = self.multipliers[slot].max(1);
            values[slot] = (rest / multiplier) as u8;
            rest %= multiplier;
        }
        values
    }
}

// ---------------------------------------------------------------------------
// Families
// ---------------------------------------------------------------------------

fn read_direction(state: &BlockState, values: &[u8], slot: usize) -> Option<Dir> {
    state.value_name(slot, values[slot]).and_then(Dir::parse)
}

fn write_direction(state: &BlockState, values: &mut [u8], slot: usize, direction: Dir) {
    if let Some(index) = state.index_of(slot, direction.name()) {
        values[slot] = index;
    }
}

fn write_name(state: &BlockState, values: &mut [u8], slot: usize, name: &str) {
    if let Some(index) = state.index_of(slot, name) {
        values[slot] = index;
    }
}

fn rotate_facing(state: &BlockState, values: &mut [u8], rotation: Rotation) {
    if let Some(slot) = state.slot("facing") {
        if let Some(current) = read_direction(state, values, slot) {
            write_direction(state, values, slot, rotation.direction(current));
        }
    }
}

fn mirror_facing(state: &BlockState, values: &mut [u8], mirror: Mirror) {
    if let Some(slot) = state.slot("facing") {
        if let Some(current) = read_direction(state, values, slot) {
            let turned = mirror.rotation(current).direction(current);
            write_direction(state, values, slot, turned);
        }
    }
}

/// The sides of fences, walls, vines, wire and tripwire.
const SIDES: [&str; 4] = ["north", "east", "south", "west"];

/// `CrossCollisionBlock.rotate` and its five twins: each side takes the value of the side
/// the turn puts in its place, and every read sees the state it started from.
fn rotate_sides(state: &BlockState, values: &mut [u8], rotation: Rotation) {
    let source: [&str; 4] = match rotation {
        Rotation::Clockwise180 => ["south", "west", "north", "east"],
        Rotation::CounterClockwise90 => ["east", "south", "west", "north"],
        Rotation::Clockwise90 => ["west", "north", "east", "south"],
        Rotation::None => return,
    };
    let mut updates = Vec::with_capacity(4);
    for (target, origin) in SIDES.iter().zip(source.iter()) {
        if let (Some(target_slot), Some(origin_slot)) = (state.slot(target), state.slot(origin)) {
            updates.push((target_slot, values[origin_slot]));
        }
    }
    for (slot, value) in updates {
        values[slot] = value;
    }
}

/// `WallBlock.mirror` and its twins: `LEFT_RIGHT` swaps north/south, `FRONT_BACK` east/west.
fn mirror_sides(state: &BlockState, values: &mut [u8], mirror: Mirror) {
    let pairs: [(&str, &str); 2] = match mirror {
        Mirror::LeftRight => [("north", "south"), ("south", "north")],
        Mirror::FrontBack => [("east", "west"), ("west", "east")],
    };
    let mut updates = Vec::with_capacity(2);
    for (target, origin) in pairs {
        if let (Some(target_slot), Some(origin_slot)) = (state.slot(target), state.slot(origin)) {
            updates.push((target_slot, values[origin_slot]));
        }
    }
    for (slot, value) in updates {
        values[slot] = value;
    }
}

/// `HugeMushroomBlock` and `MultifaceBlock`: every face takes the value of the face the
/// turn brings to it.
fn rotate_faces(state: &BlockState, values: &mut [u8], rotation: Rotation) {
    let mut updates = Vec::with_capacity(6);
    for direction in DIRECTIONS {
        let target = state.slot(rotation.direction(direction).name());
        let origin = state.slot(direction.name());
        if let (Some(target_slot), Some(origin_slot)) = (target, origin) {
            updates.push((target_slot, values[origin_slot]));
        }
    }
    for (slot, value) in updates {
        values[slot] = value;
    }
}

fn mirror_faces(state: &BlockState, values: &mut [u8], mirror: Mirror) {
    let mut updates = Vec::with_capacity(6);
    for direction in DIRECTIONS {
        let target = state.slot(mirror.direction(direction).name());
        let origin = state.slot(direction.name());
        if let (Some(target_slot), Some(origin_slot)) = (target, origin) {
            updates.push((target_slot, values[origin_slot]));
        }
    }
    for (slot, value) in updates {
        values[slot] = value;
    }
}

/// `BaseRailBlock.rotate`: the ten rail shapes under each quarter turn.
fn rail_rotate(shape: &str, rotation: Rotation) -> Option<&'static str> {
    Some(match rotation {
        Rotation::None => return None,
        Rotation::Clockwise180 => match shape {
            "ascending_east" => "ascending_west",
            "ascending_west" => "ascending_east",
            "ascending_north" => "ascending_south",
            "ascending_south" => "ascending_north",
            "south_east" => "north_west",
            "south_west" => "north_east",
            "north_west" => "south_east",
            "north_east" => "south_west",
            "north_south" => return None,
            "east_west" => return None,
            _ => return None,
        },
        Rotation::CounterClockwise90 => match shape {
            "ascending_east" => "ascending_north",
            "ascending_west" => "ascending_south",
            "ascending_north" => "ascending_west",
            "ascending_south" => "ascending_east",
            "north_south" => "east_west",
            "east_west" => "north_south",
            "south_east" => "north_east",
            "south_west" => "south_east",
            "north_west" => "south_west",
            "north_east" => "north_west",
            _ => return None,
        },
        Rotation::Clockwise90 => match shape {
            "ascending_east" => "ascending_south",
            "ascending_west" => "ascending_north",
            "ascending_north" => "ascending_east",
            "ascending_south" => "ascending_west",
            "north_south" => "east_west",
            "east_west" => "north_south",
            "south_east" => "south_west",
            "south_west" => "north_west",
            "north_west" => "north_east",
            "north_east" => "south_east",
            _ => return None,
        },
    })
}

/// `BaseRailBlock.mirror`: `LEFT_RIGHT` flips the north/south ends, `FRONT_BACK` the
/// east/west ones.
fn rail_mirror(shape: &str, mirror: Mirror) -> Option<&'static str> {
    Some(match mirror {
        Mirror::LeftRight => match shape {
            "ascending_north" => "ascending_south",
            "ascending_south" => "ascending_north",
            "south_east" => "north_east",
            "south_west" => "north_west",
            "north_west" => "south_west",
            "north_east" => "south_east",
            _ => return None,
        },
        Mirror::FrontBack => match shape {
            "ascending_east" => "ascending_west",
            "ascending_west" => "ascending_east",
            "south_east" => "south_west",
            "south_west" => "south_east",
            "north_west" => "north_east",
            "north_east" => "north_west",
            _ => return None,
        },
    })
}

/// A shape valued property, mapped by name.
fn map_shape(
    state: &BlockState,
    values: &mut [u8],
    property: &str,
    map: impl Fn(&str) -> Option<&'static str>,
) {
    let Some(slot) = state.slot(property) else {
        return;
    };
    let Some(name) = state.value_name(slot, values[slot]) else {
        return;
    };
    if let Some(mapped) = map(name) {
        write_name(state, values, slot, mapped);
    }
}

/// `FrontAndTop`: `front_top`, both directions turned by the octahedral group the rotation
/// or mirror names.
fn turn_orientation(
    state: &BlockState,
    values: &mut [u8],
    property: &str,
    turn: impl Fn(Dir) -> Dir,
) {
    let Some(slot) = state.slot(property) else {
        return;
    };
    let Some(name) = state.value_name(slot, values[slot]) else {
        return;
    };
    let mut parts = name.split('_');
    let (Some(front), Some(top)) = (parts.next(), parts.next()) else {
        return;
    };
    let (Some(front), Some(top)) = (Dir::parse(front), Dir::parse(top)) else {
        return;
    };
    let wanted = format!("{}_{}", turn(front).name(), turn(top).name());
    write_name(state, values, slot, &wanted);
}

/// `RotatedPillarBlock.rotatePillar`: a quarter turn swaps the X and Z axes.
fn rotate_axis(state: &BlockState, values: &mut [u8], rotation: Rotation) {
    let Some(slot) = state.slot("axis") else {
        return;
    };
    let Some(name) = state.value_name(slot, values[slot]) else {
        return;
    };
    let axis = match name {
        "x" => Axis::X,
        "y" => Axis::Y,
        "z" => Axis::Z,
        _ => return,
    };
    let turned = rotation.axis(axis);
    let wanted = match turned {
        Axis::X => "x",
        Axis::Y => "y",
        Axis::Z => "z",
    };
    write_name(state, values, slot, wanted);
}

// ---------------------------------------------------------------------------
// Building the table
// ---------------------------------------------------------------------------

/// The block's class chain, starting with the block's own class.
fn class_chain(row: &block::Block) -> Vec<String> {
    let mut chain: Vec<String> = Vec::with_capacity(row.chain.len() + 1);
    if row.chain.first() != Some(&row.class) {
        chain.push(row.class.clone());
    }
    chain.extend(row.chain.iter().cloned());
    chain
}

/// The block entity class a block creates: the `new` in the nearest `newBlockEntity`.
fn block_entity_class(classes: &Classes, row: &block::Block) -> Option<String> {
    for name in class_chain(row) {
        let Some(file) = classes.class(&name) else {
            continue;
        };
        let Some(method) = file.methods.iter().find(|method| {
            method.name == "newBlockEntity" && method.descriptor == NEW_BLOCK_ENTITY
        }) else {
            continue;
        };
        let code = method.code.as_ref()?;
        for instruction in decode(&code.bytes) {
            if instruction.opcode == 0xbb {
                if let Some(class) = instruction
                    .index
                    .and_then(|index| class_at(&file.constants, index))
                {
                    return Some(class);
                }
            }
        }
        return None;
    }
    None
}

/// `BlockEntityTypeIds`: field name -> the registry id it names.
const ENTITY_TYPE_IDS: &str = "net/minecraft/world/level/block/entity/BlockEntityTypeIds";

/// `BlockEntityTypeIds`: field name -> `minecraft:<id>`.
fn entity_type_ids(classes: &Classes) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(file) = classes.class(ENTITY_TYPE_IDS) else {
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
                let Some((owner, field)) = instruction
                    .index
                    .and_then(|index| field_at(&file.constants, index))
                else {
                    continue;
                };
                if owner == ENTITY_TYPE_IDS {
                    if let Some(id) = id.take() {
                        out.insert(field, format!("minecraft:{id}"));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// `BlockEntityTypes`: block entity class -> `minecraft:<type id>`.
fn entity_types(classes: &Classes) -> Result<BTreeMap<String, String>, String> {
    let file = classes
        .class(ENTITY_TYPES)
        .ok_or_else(|| format!("{ENTITY_TYPES}: no such class"))?;
    let method = file
        .methods
        .iter()
        .find(|method| method.name == "<clinit>")
        .ok_or_else(|| format!("{ENTITY_TYPES}: no static initialiser"))?;
    let code = method
        .code
        .as_ref()
        .ok_or_else(|| format!("{ENTITY_TYPES}: no initialiser code"))?;
    let ids = entity_type_ids(classes);
    let mut out = BTreeMap::new();
    let mut id: Option<String> = None;
    let mut entity: Option<String> = None;
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
            0xb2 => {
                // `getstatic BlockEntityTypeIds.X`: the type's id, by field name.
                if let Some((owner, field)) = instruction
                    .index
                    .and_then(|index| field_at(&file.constants, index))
                {
                    if owner == ENTITY_TYPE_IDS {
                        id = ids.get(&field).cloned();
                    }
                }
            }
            0xba => {
                if entity.is_none() {
                    entity = instruction
                        .index
                        .and_then(|index| constructor_class(&file, index));
                }
            }
            0xb3 => {
                let Some((owner, _field)) = instruction
                    .index
                    .and_then(|index| field_at(&file.constants, index))
                else {
                    continue;
                };
                if owner == ENTITY_TYPES {
                    if let (Some(id), Some(entity)) = (id.take(), entity.take()) {
                        out.insert(entity, id);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Whether a block entity class is a `RandomizableContainer` — the class that lets a
/// structure template put a loot table in it.
fn randomizable(classes: &Classes, entity: &str) -> bool {
    let mut current = Some(entity.to_string());
    while let Some(name) = current {
        if name == RANDOMIZABLE {
            return true;
        }
        let Some(file) = classes.class(&name) else {
            return false;
        };
        if let Some(bytes) = classes.files.get(&file.name) {
            if interfaces(bytes)
                .map(|list| {
                    list.iter()
                        .any(|interface| interface == RANDOMIZABLE_INTERFACE)
                })
                .unwrap_or(false)
            {
                return true;
            }
        }
        current = Some(file.super_name.clone()).filter(|super_name| !super_name.is_empty());
    }
    false
}

/// Builds the whole table from the player's jar.
pub fn table(jar_path: &Path) -> Result<Table, String> {
    let declared = states::state_table(jar_path)?;
    let registry = block::registry(jar_path)?;
    if declared.blocks.len() != registry.len() {
        return Err(format!(
            "the state table has {} blocks, the registry {}",
            declared.blocks.len(),
            registry.len()
        ));
    }
    let classes = Classes::open(jar_path)?;
    let mut blocks = Vec::with_capacity(declared.blocks.len());
    let mut start = 0u32;
    for declared in &declared.blocks {
        let sorted = declared.sorted();
        let mut multipliers = vec![0usize; sorted.len()];
        let mut running = 1usize;
        for slot in (0..sorted.len()).rev() {
            multipliers[slot] = running;
            running *= sorted[slot].values.len();
        }
        blocks.push(BlockState {
            id: declared.id.clone(),
            properties: sorted
                .iter()
                .map(|property| (property.name.clone(), property.values.clone()))
                .collect(),
            multipliers,
            start,
            count: declared.state_count as u32,
        });
        start += declared.state_count as u32;
    }
    let total = start as usize;
    let mut rotate = Vec::with_capacity(total);
    let mut mirror = Vec::with_capacity(total);
    for (index, definition) in blocks.iter().enumerate() {
        let rotate_class = declaring_class(&classes, &registry[index], "rotate");
        let mirror_class = declaring_class(&classes, &registry[index], "mirror");
        for state in 0..definition.count {
            let id = definition.start + state;
            let values = definition.values_from(id);
            rotate.push(rotate_state(definition, &rotate_class, &values)?);
            mirror.push(mirror_state(definition, &mirror_class, &values)?);
        }
    }
    let types = entity_types(&classes)?;
    let mut block_entities: Vec<(String, Vec<u32>)> = Vec::new();
    let mut positions: BTreeMap<String, usize> = BTreeMap::new();
    let mut loot_containers: Vec<u32> = Vec::new();
    for (index, definition) in blocks.iter().enumerate() {
        let Some(entity) = block_entity_class(&classes, &registry[index]) else {
            continue;
        };
        let Some(kind) = types.get(&entity) else {
            return Err(format!(
                "{}: {} has no block entity type in BlockEntityTypes",
                definition.id,
                short(&entity)
            ));
        };
        let states: Vec<u32> = (definition.start..definition.start + definition.count).collect();
        if randomizable(&classes, &entity) {
            loot_containers.extend(states.iter().copied());
        }
        match positions.get(kind) {
            Some(slot) => block_entities[*slot].1.extend(states),
            None => {
                positions.insert(kind.clone(), block_entities.len());
                block_entities.push((kind.clone(), states));
            }
        }
    }
    Ok(Table {
        states: total,
        rotate,
        mirror,
        full_block: vec![false; total],
        loot_containers,
        block_entities,
    })
}

fn rotate_state(state: &BlockState, class: &str, values: &[u8]) -> Result<[u32; 3], String> {
    let mut out = [0u32; 3];
    for (slot, rotation) in ROTATIONS.into_iter().enumerate() {
        let mut turned = values.to_vec();
        rotate_with(state, class, &mut turned, rotation)?;
        out[slot] = state.id_of(&turned);
    }
    Ok(out)
}

fn mirror_state(state: &BlockState, class: &str, values: &[u8]) -> Result<[u32; 2], String> {
    let mut out = [0u32; 2];
    for (slot, mirror) in [Mirror::LeftRight, Mirror::FrontBack]
        .into_iter()
        .enumerate()
    {
        let mut flipped = values.to_vec();
        mirror_with(state, class, &mut flipped, mirror)?;
        out[slot] = state.id_of(&flipped);
    }
    Ok(out)
}

fn rotate_with(
    state: &BlockState,
    class: &str,
    values: &mut [u8],
    rotation: Rotation,
) -> Result<(), String> {
    match class {
        // The base class and the classes that keep its identity turn.
        "BlockBehaviour" | "Block" => {}
        "HorizontalDirectionalBlock"
        | "AbstractFurnaceBlock"
        | "AmethystClusterBlock"
        | "AnvilBlock"
        | "AttachedStemBlock"
        | "BarrelBlock"
        | "BaseCoralWallFanBlock"
        | "BeehiveBlock"
        | "BellBlock"
        | "CalibratedSculkSensorBlock"
        | "CampfireBlock"
        | "ChestBlock"
        | "ChiseledBookShelfBlock"
        | "CommandBlock"
        | "CopperGolemStatueBlock"
        | "DecoratedPotBlock"
        | "DispenserBlock"
        | "DoorBlock"
        | "EndPortalFrameBlock"
        | "EnderChestBlock"
        | "FlowerBedBlock"
        | "GrindstoneBlock"
        | "HopperBlock"
        | "LadderBlock"
        | "LeafLitterBlock"
        | "LecternBlock"
        | "MovingPistonBlock"
        | "ObserverBlock"
        | "PistonBaseBlock"
        | "PistonHeadBlock"
        | "RedstoneWallTorchBlock"
        | "RodBlock"
        | "ShelfBlock"
        | "ShulkerBoxBlock"
        | "SmallDripleafBlock"
        | "StairBlock"
        | "StonecutterBlock"
        | "TripWireHookBlock"
        | "VaultBlock"
        | "WallBannerBlock"
        | "WallHangingSignBlock"
        | "WallSignBlock"
        | "WallSkullBlock"
        | "WallTorchBlock" => rotate_facing(state, values, rotation),
        "BannerBlock" | "CeilingHangingSignBlock" | "StandingSignBlock" | "SkullBlock" => {
            let Some(slot) = state.slot("rotation") else {
                return Ok(());
            };
            let Some(name) = state.value_name(slot, values[slot]) else {
                return Ok(());
            };
            let Ok(value) = name.parse::<i32>() else {
                return Ok(());
            };
            let max = state.properties[slot].1.len() as i32;
            write_name(state, values, slot, &rotation.value(value, max).to_string());
        }
        "RotatedPillarBlock" | "InfestedRotatedPillarBlock" | "CreakingHeartBlock" => {
            rotate_axis(state, values, rotation)
        }
        "NetherPortalBlock" => {
            if let Some(slot) = state.slot("axis") {
                if let Some(name) = state.value_name(slot, values[slot]) {
                    let axis = if name == "x" { Axis::X } else { Axis::Z };
                    let wanted = if rotation.axis(axis) == Axis::X {
                        "x"
                    } else {
                        "z"
                    };
                    write_name(state, values, slot, wanted);
                }
            }
        }
        "CrossCollisionBlock"
        | "MossyCarpetBlock"
        | "RedstoneWireBlock"
        | "TripWireBlock"
        | "VineBlock"
        | "WallBlock" => rotate_sides(state, values, rotation),
        "HugeMushroomBlock" | "MultifaceBlock" => rotate_faces(state, values, rotation),
        "RailBlock" | "PoweredRailBlock" | "DetectorRailBlock" => {
            map_shape(state, values, "shape", |shape| rail_rotate(shape, rotation))
        }
        "CrafterBlock" | "JigsawBlock" => {
            turn_orientation(state, values, "orientation", |direction| {
                rotation.direction(direction)
            });
        }
        other => {
            return Err(format!(
                "{}: no rotate for block class {other}; this exporter does not know that \
                 class's turn",
                state.id
            ))
        }
    }
    Ok(())
}

fn mirror_with(
    state: &BlockState,
    class: &str,
    values: &mut [u8],
    mirror: Mirror,
) -> Result<(), String> {
    match class {
        "BlockBehaviour" | "Block" => {}
        // `state.rotate(Mirror.getRotation(facing))` — the ~40 classes that repeat it.
        "HorizontalDirectionalBlock"
        | "AbstractFurnaceBlock"
        | "AmethystClusterBlock"
        | "AttachedStemBlock"
        | "BarrelBlock"
        | "BaseCoralWallFanBlock"
        | "BeehiveBlock"
        | "BellBlock"
        | "CalibratedSculkSensorBlock"
        | "CampfireBlock"
        | "ChestBlock"
        | "ChiseledBookShelfBlock"
        | "CommandBlock"
        | "CopperGolemStatueBlock"
        | "DecoratedPotBlock"
        | "DispenserBlock"
        | "EndPortalFrameBlock"
        | "EnderChestBlock"
        | "FlowerBedBlock"
        | "GrindstoneBlock"
        | "HopperBlock"
        | "LadderBlock"
        | "LeafLitterBlock"
        | "LecternBlock"
        | "MovingPistonBlock"
        | "ObserverBlock"
        | "PistonBaseBlock"
        | "PistonHeadBlock"
        | "RedstoneWallTorchBlock"
        | "ShelfBlock"
        | "ShulkerBoxBlock"
        | "SmallDripleafBlock"
        | "StonecutterBlock"
        | "TripWireHookBlock"
        | "VaultBlock"
        | "WallBannerBlock"
        | "WallHangingSignBlock"
        | "WallSignBlock"
        | "WallSkullBlock"
        | "WallTorchBlock" => mirror_facing(state, values, mirror),
        "RodBlock" => {
            if let Some(slot) = state.slot("facing") {
                if let Some(current) = read_direction(state, values, slot) {
                    write_direction(state, values, slot, mirror.direction(current));
                }
            }
        }
        "BannerBlock" | "CeilingHangingSignBlock" | "StandingSignBlock" | "SkullBlock" => {
            let Some(slot) = state.slot("rotation") else {
                return Ok(());
            };
            let Some(name) = state.value_name(slot, values[slot]) else {
                return Ok(());
            };
            let Ok(value) = name.parse::<i32>() else {
                return Ok(());
            };
            let max = state.properties[slot].1.len() as i32;
            write_name(state, values, slot, &mirror.value(value, max).to_string());
        }
        "CrossCollisionBlock"
        | "MossyCarpetBlock"
        | "RedstoneWireBlock"
        | "TripWireBlock"
        | "VineBlock"
        | "WallBlock" => mirror_sides(state, values, mirror),
        "HugeMushroomBlock" | "MultifaceBlock" => mirror_faces(state, values, mirror),
        "RailBlock" | "PoweredRailBlock" | "DetectorRailBlock" => {
            map_shape(state, values, "shape", |shape| rail_mirror(shape, mirror))
        }
        "StairBlock" => mirror_stairs(state, values, mirror),
        "DoorBlock" => {
            // `state.rotate(Mirror.getRotation(facing)).cycle(HINGE)`
            if let Some(slot) = state.slot("facing") {
                if let Some(current) = read_direction(state, values, slot) {
                    let turned = mirror.rotation(current).direction(current);
                    write_direction(state, values, slot, turned);
                }
            }
            if let Some(slot) = state.slot("hinge") {
                let size = state.properties[slot].1.len() as u8;
                values[slot] = (values[slot] + 1) % size;
            }
        }
        "CrafterBlock" | "JigsawBlock" => {
            turn_orientation(state, values, "orientation", |direction| {
                mirror.direction(direction)
            });
        }
        other => {
            return Err(format!(
                "{}: no mirror for block class {other}; this exporter does not know that \
                 class's flip",
                state.id
            ))
        }
    }
    Ok(())
}

/// `StairBlock.mirror`: `LEFT_RIGHT` turns stairs that face along Z, `FRONT_BACK` those
/// that face along X. Both half-turn the stair, and the shape swaps: `LEFT_RIGHT` swaps
/// both the inner and the outer pair, `FRONT_BACK` only the outer pair (vanilla's own
/// asymmetry). Any other stair keeps the base class's identity.
fn mirror_stairs(state: &BlockState, values: &mut [u8], mirror: Mirror) {
    let Some(facing_slot) = state.slot("facing") else {
        return;
    };
    let Some(facing) = read_direction(state, values, facing_slot) else {
        return;
    };
    let turns = match mirror {
        Mirror::LeftRight => facing.axis() == Axis::Z,
        Mirror::FrontBack => facing.axis() == Axis::X,
    };
    if !turns {
        return;
    }
    write_direction(state, values, facing_slot, facing.opposite());
    let Some(shape_slot) = state.slot("shape") else {
        return;
    };
    let Some(name) = state.value_name(shape_slot, values[shape_slot]) else {
        return;
    };
    let swapped = match (mirror, name) {
        (_, "straight") => return,
        (Mirror::LeftRight, "inner_left") => "inner_right",
        (Mirror::LeftRight, "inner_right") => "inner_left",
        (_, "outer_left") => "outer_right",
        (_, "outer_right") => "outer_left",
        _ => return,
    };
    write_name(state, values, shape_slot, swapped);
}

// ---------------------------------------------------------------------------
// Which class answers rotate/mirror
// ---------------------------------------------------------------------------

/// The class that answers `rotate`/`mirror` for a block: the first class in its chain that
/// declares the method, or `BlockBehaviour` (identity) when nothing does.
fn declaring_class(classes: &Classes, row: &block::Block, method: &str) -> String {
    for name in class_chain(row) {
        let Some(file) = classes.class(&name) else {
            continue;
        };
        if file
            .methods
            .iter()
            .any(|candidate| candidate.name == method)
        {
            return short(&file.name).to_string();
        }
    }
    "BlockBehaviour".to_string()
}

/// The jar's classes, parsed on demand.
struct Classes {
    files: BTreeMap<String, Vec<u8>>,
    by_short: BTreeMap<String, String>,
    parsed: RefCell<BTreeMap<String, Option<Rc<ClassFile>>>>,
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
        Ok(Classes {
            files,
            by_short,
            parsed: RefCell::new(BTreeMap::new()),
        })
    }

    fn class(&self, name: &str) -> Option<Rc<ClassFile>> {
        let internal = if name.contains('/') {
            self.files.get_key_value(name)?.0.clone()
        } else {
            self.by_short.get(name)?.clone()
        };
        if let Some(hit) = self.parsed.borrow().get(&internal) {
            return hit.clone();
        }
        let parsed = self
            .files
            .get(&internal)
            .and_then(|bytes| classfile::parse(bytes).ok())
            .map(Rc::new);
        self.parsed.borrow_mut().insert(internal, parsed.clone());
        parsed
    }
}

fn short(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

// ---------------------------------------------------------------------------
// Block entities: block -> entity class -> entity type id
// ---------------------------------------------------------------------------

const RANDOMIZABLE: &str =
    "net/minecraft/world/level/block/entity/RandomizableContainerBlockEntity";
const RANDOMIZABLE_INTERFACE: &str = "net/minecraft/world/RandomizableContainer";
const ENTITY_TYPES: &str = "net/minecraft/world/level/block/entity/BlockEntityTypes";
const NEW_BLOCK_ENTITY: &str = "(Lnet/minecraft/core/BlockPos;Lnet/minecraft/world/level/block/state/BlockState;)Lnet/minecraft/world/level/block/entity/BlockEntity;";

/// One decoded instruction: its opcode and, where the module reads it, the constant index.
struct Instruction {
    opcode: u8,
    index: Option<usize>,
}

/// Decode a method body. Only lengths and the operands this module reads are kept;
/// `tableswitch`/`lookupswitch` are skipped by their own length so the scan cannot
/// desynchronise (the same shape `block.rs` uses).
fn decode(bytes: &[u8]) -> Vec<Instruction> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let opcode = bytes[at];
        let mut index = None;
        let length = match opcode {
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
        out.push(Instruction { opcode, index });
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

/// The class name behind a `Constant::Class` pool index.
fn class_at(constants: &[classfile::Constant], index: usize) -> Option<String> {
    match constants.get(index)? {
        classfile::Constant::Class(reference) => resolve(constants, reference),
        _ => None,
    }
}

/// `(owner, field name)` behind a `putstatic`/`getstatic` pool index.
fn field_at(constants: &[classfile::Constant], index: usize) -> Option<(String, String)> {
    let (class, name_and_type) = match constants.get(index)? {
        classfile::Constant::Fieldref { class, name, .. } => (class, name),
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
    let field = match constants.get(name_index)? {
        classfile::Constant::NameAndType { name, .. } => resolve(constants, name)?,
        _ => return None,
    };
    Some((owner, field))
}

/// The implementation class a `invokedynamic` constructor reference names, for
/// `BlockEntityType.Builder.of(XxxBlockEntity::new, ...)`.
fn constructor_class(class: &ClassFile, index: usize) -> Option<String> {
    let bootstrap = match class.constants.get(index)? {
        classfile::Constant::InvokeDynamic { bootstrap, .. } => *bootstrap,
        _ => return None,
    };
    let row = class.bootstraps.get(bootstrap)?;
    for argument in &row.arguments {
        let reference = match class.constants.get(*argument)? {
            classfile::Constant::MethodHandle { reference } => *reference,
            _ => continue,
        };
        if let Some(classfile::Constant::Methodref { class: owner, .. }) =
            class.constants.get(reference)
        {
            let index: usize = owner.trim_start_matches('#').parse().ok()?;
            return class_at(&class.constants, index);
        }
    }
    None
}

/// The `interface` list of a class file, which the shared parser does not keep.
///
/// Everything before it is the constant pool; the tags decide each entry's width.
fn interfaces(bytes: &[u8]) -> Option<Vec<String>> {
    let mut at = 10usize; // magic, minor, major, constant-pool count
    let count = u16::from_be_bytes([*bytes.get(8)?, *bytes.get(9)?]) as usize;
    let mut pool: Vec<Option<String>> = vec![None; count];
    let mut index = 1;
    while index < count {
        let tag = *bytes.get(at)?;
        at += 1;
        match tag {
            1 => {
                let length = u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]) as usize;
                at += 2;
                pool[index] =
                    Some(String::from_utf8_lossy(bytes.get(at..at + length)?).into_owned());
                at += length;
            }
            7 => {
                pool[index] = Some(format!(
                    "#{}",
                    u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?])
                ));
                at += 2;
            }
            8 | 16 | 19 | 20 => at += 2,
            3 | 4 | 9 | 10 | 11 | 12 | 17 | 18 => at += 4,
            5 | 6 => {
                at += 8;
                index += 1;
            }
            15 => at += 3,
            _ => return None,
        }
        index += 1;
    }
    at += 6; // access flags, this class, super class
    let count = u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]) as usize;
    at += 2;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let class = u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]) as usize;
        at += 2;
        let reference = pool.get(class)?.clone()?;
        let inner: usize = reference.trim_start_matches('#').parse().ok()?;
        out.push(pool.get(inner)?.clone()?);
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------------

/// Renders the table the way Gson writes it: compact, keys in the exporter's order.
pub fn render(table: &Table) -> String {
    let mut out = String::with_capacity(table.states * 40 + 64);
    out.push_str("{\"states\":");
    out.push_str(&table.states.to_string());
    out.push_str(",\"rotate\":[");
    for (index, row) in table.rotate.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('[');
        out.push_str(&row[0].to_string());
        out.push(',');
        out.push_str(&row[1].to_string());
        out.push(',');
        out.push_str(&row[2].to_string());
        out.push(']');
    }
    out.push_str("],\"mirror\":[");
    for (index, row) in table.mirror.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('[');
        out.push_str(&row[0].to_string());
        out.push(',');
        out.push_str(&row[1].to_string());
        out.push(']');
    }
    out.push_str("],\"full_block\":[");
    for (index, flag) in table.full_block.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(if *flag { "true" } else { "false" });
    }
    out.push_str("],\"loot_containers\":[");
    for (index, state) in table.loot_containers.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&state.to_string());
    }
    out.push_str("],\"block_entities\":{");
    for (index, (kind, members)) in table.block_entities.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('"');
        out.push_str(kind);
        out.push_str("\":[");
        for (position, state) in members.iter().enumerate() {
            if position > 0 {
                out.push(',');
            }
            out.push_str(&state.to_string());
        }
        out.push(']');
    }
    out.push_str("}}");
    out
}

/// Builds the table and writes it below `root`.
pub fn write(jar_path: &Path, root: &Path) -> Result<Coverage, String> {
    let table = table(jar_path)?;
    let path = root.join(TABLE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(&path, render(&table)).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut coverage = Coverage::default();
    coverage.produced.push(TABLE.to_string());
    coverage.missing.push(format!("{TABLE}#full_block"));
    coverage.notes.push(
        "full_block needs each state's collision shape (`isCollisionShapeFullBlock`), \
         which is block geometry rather than a value this exporter can read. Every other \
         section, states included, is exact."
            .to_string(),
    );
    coverage.notes.push(
        "block_entities carries one type the prepared copy does not, `minecraft:vault` for \
         `VaultBlockEntity` (32 states): the jar registers it and `VaultBlock.newBlockEntity` \
         creates it, so the jar's answer includes it."
            .to_string(),
    );
    Ok(coverage)
}
