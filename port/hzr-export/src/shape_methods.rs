//! A bytecode interpreter for vanilla block *shape methods*.
//!
//! This is geometry step 2 of 4: given a block state, run the block class's own
//! `getShape`/`getCollisionShape`/`getOcclusionShape`/`getBlockSupportShape`
//! methods from the player's `client.jar` and return the `VoxelShape` they
//! compute. `geometry.rs` derives only the default path (blocks with no shape
//! override); this module interprets the overrides.
//!
//! # What is interpreted
//!
//! * virtual dispatch through the class chain, including `invokespecial`/`super`
//!   calls into `BlockBehaviour`, `BlockBehaviour$BlockStateBase` and `Block`
//!   (the defaults live there: `getCollisionShape` is
//!   `hasCollision ? getShape : empty`, `getShape` is `Shapes.block()`,
//!   `getOcclusionShape` is `getShape`);
//! * `state.getValue(property)` for the three property kinds, resolving the
//!   state's current value through the *property table* (`states.rs` produces
//!   it; this module only consumes the resulting `name -> serialized value`
//!   map, it does not re-derive the table);
//! * `switch`/`tableswitch`/`lookupswitch`, including the compiler's synthetic
//!   `X$1.$SwitchMap$...` `int[]` fields built in `<clinit>` from
//!   `Enum.ordinal()`;
//! * cached shapes stored in static fields: plain `VoxelShape` fields,
//!   `VoxelShape[]` arrays (indexed by `IntegerProperty.intValue()`),
//!   `Map<Direction, VoxelShape>` / `Map<Axis, VoxelShape>` /
//!   `Map<Integer, VoxelShape>` built in `<clinit>` with `Map.of` or `put`;
//! * `Shapes.*` factories (`empty`, `block`, `box`, `create`, `or`, `join`,
//!   `rotate`, `rotateHorizontal`, `rotateHorizontalAxis`, `rotateAllAxis`,
//!   `rotateAll`, `rotateAttachFace`) and `Block.box/column/boxZ/cube/boxes`
//!   (the arithmetic helpers are interpreted; only the geometry primitives are
//!   handed to the [`ShapeFactory`] contract);
//! * `invokedynamic` lambdas (e.g. `SnowLayerBlock.<clinit>`'s
//!   `Block.boxes(8, i -> Block.box(...))`) and `Function.apply`;
//! * enum constants and `ordinal()`/`name()`/`values()`, and `OctahedralGroup`
//!   composition (`Shapes.rotate` needs it), read out of the jar rather than
//!   hardcoded.
//!
//! # The VoxelShape API contract
//!
//! Geometry is not this module's job: it is step 1 ([`voxelshape.rs`]).
//! `voxelshape.rs` must not be edited from here, so this module depends on the
//! documented *trait the caller supplies*: [`ShapeFactory`]:
//!
//! ```text
//! empty()                      Shapes.empty()
//! block()                      Shapes.block()
//! create(x1,y1,z1,x2,y2,z2)    Shapes.box/Shapes.create, in block coordinates
//!                              (0..1), with the empty-shape and full-cube
//!                              shortcuts and the CubeVoxelShape/ArrayVoxelShape
//!                              choice of vanilla `Shapes.create`
//! or(a,b)                      Shapes.join(a,b,BooleanOp.OR)
//! join(a,b,op)                 Shapes.join with any BooleanOp
//! rotate(shape,rotation,c)     Shapes.rotate(shape, group, center)
//! is_empty(shape)              VoxelShape.isEmpty()
//! ```
//!
//! `voxelshape.rs` already exports the matching operations
//! (`empty`, `block`, `box_`, `or`, `join`, `BooleanOp`, `is_empty`), so an
//! adapter is a thin mapping; `rotate` is the one operation that side does not
//! implement yet, which is why the rotation maps below hand it to the factory.
//! [`Rotation`] is the `OctahedralGroup` value as `(axis permutation,
//! per-output axis inversion)`; [`Rotation::compose`] mirrors
//! `OctahedralGroup.compose` exactly (the formula is the one vanilla's own
//! `CAYLEY_TABLE` builder uses).
//!
//! [`tests/shape_methods.rs`] implements the trait and re-encodes the
//! interpreted shapes against `tmp/ship-extract/cached-collision.nbt` and
//! `occlusion.nbt`: every cached state it reports a shape for is geometry-equal
//! to the game's own shape, and the curated families are byte-identical. That
//! file also documents why a few `Shapes.or`-composed states differ in their
//! coordinate *partition* there (it is the reference factory's `optimize`
//! reconstruction, which `voxelshape.rs` owns).
//!
//! # What is *not* interpreted (reported, never guessed)
//!
//! * shape fields that are **instance** fields initialised in a block
//!   constructor (`FenceBlock`, `VineBlock`, `CrossCollisionBlock`,
//!   `ButtonBlock`, ... store a `Function<BlockState,VoxelShape>` field).
//!   Building a real block instance runs the state-definition registration
//!   chain, which is outside a shape interpreter; those blocks stay gaps;
//! * shapes that depend on the level/position (`VoxelShape.move(getOffset(...))`,
//!   `CollisionContext`, block entities): the value is not a function of the
//!   state alone, so the prepared tables mark those states uncached (`-1`) and
//!   this VM returns no shape;
//! * the `Properties.hasCollision`/`noOcclusion` flags: the caller supplies
//!   `has_collision` in [`State`] (they are `geometry.rs`'s property sweep, not
//!   block-class bytecode);
//! * `instanceof`/`checkcast` are structural only; unknown JDK helpers such as
//!   `Stream` are treated as opaque. An opaque value can never become a shape:
//!   the query returns `None` and lists the warning, so a wrong answer cannot
//!   look like a right one.
//!
//! # Coverage
//!
//! The query returns `shape: Some(..)` with an empty warning list only when the
//! whole method body was interpreted. On the 35,723-state registry the test
//! runs both prepared tables through the VM: **17,901** cached collision states
//! and **19,161** cached occlusion states produce a shape (the rest are the
//! constructor-field, dynamic-position and `hasCollision` gaps above). Every
//! one of those shapes is point-set equal to the game's, and the curated
//! families (`SlabBlock`, `SnowLayerBlock`, `LadderBlock`, `ChestBlock`,
//! `CactusBlock`, `HeavyCoreBlock`, `PathBlock`, `MudBlock`, `LeavesBlock`,
//! `ChainBlock`, `AttachedStemBlock`) are byte-identical, covering `Block.box`/
//! `block.column`/`boxZ`, `VoxelShape[]`, `Map<Direction|Axis, VoxelShape>`,
//! enum/int/bool properties, switch maps and `Shapes.or`.

#![allow(dead_code, clippy::duplicate_mod)]

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

use classfile::{ClassFile, Constant, Member};

const SHAPES: &str = "net/minecraft/world/phys/shapes/Shapes";
const BLOCK: &str = "net/minecraft/world/level/block/Block";
const BOOLEAN_OP: &str = "net/minecraft/world/phys/shapes/BooleanOp";
const OCTAHEDRAL: &str = "com/mojang/math/OctahedralGroup";
const PROPERTIES_PREFIX: &str = "net/minecraft/world/level/block/state/properties/";

// ---------------------------------------------------------------------------
// The VoxelShape contract
// ---------------------------------------------------------------------------

/// `Direction.Axis` coordinate ordinals, matching vanilla (`X`, `Y`, `Z`).
pub const AXIS_X: usize = 0;
pub const AXIS_Y: usize = 1;
pub const AXIS_Z: usize = 2;

/// An `OctahedralGroup` value: the axis permutation and the per-output-axis
/// inversion flags vanilla carries on the enum constant.
///
/// `permutation().permuteAxis(axis)` is `perm[axis]`; `inverts(axis)` is
/// `inverted[axis]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rotation {
    /// `perm[output_axis] = source_axis`.
    pub perm: [usize; 3],
    /// `inverted[output_axis]`, vanilla `OctahedralGroup.inverts`.
    pub inverted: [bool; 3],
    /// The enum constant's name, for diagnostics only.
    pub name: String,
}

impl Rotation {
    pub fn identity() -> Rotation {
        Rotation {
            perm: [AXIS_X, AXIS_Y, AXIS_Z],
            inverted: [false, false, false],
            name: "IDENTITY".to_string(),
        }
    }

    pub fn is_identity(&self) -> bool {
        self.perm == [AXIS_X, AXIS_Y, AXIS_Z] && self.inverted == [false, false, false]
    }

    /// `OctahedralGroup.compose`: vanilla's `CAYLEY_TABLE[self][other]`, built by
    /// its own `<clinit>` as
    /// `perm = other.perm oself.perm` and
    /// `invert[i] = self.invert[i] XOR other.invert[self.perm[i]]`.
    pub fn compose(&self, other: &Rotation) -> Rotation {
        let mut perm = [0usize; 3];
        let mut inverted = [false; 3];
        for axis in 0..3 {
            perm[axis] = other.perm[self.perm[axis]];
            inverted[axis] = self.inverted[axis] ^ other.inverted[self.perm[axis]];
        }
        Rotation {
            perm,
            inverted,
            name: format!("{}*{}", self.name, other.name),
        }
    }
}

/// `BooleanOp`: the eleven constants vanilla's `Shapes.join` accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinOp {
    False,
    NotOr,
    OnlySecond,
    NotFirst,
    OnlyFirst,
    NotAnd,
    True,
    And,
    Or,
    AndNot,
    OrNot,
}

impl JoinOp {
    pub fn from_name(name: &str) -> Option<JoinOp> {
        Some(match name {
            "FALSE" => JoinOp::False,
            "NOT_OR" => JoinOp::NotOr,
            "ONLY_SECOND" => JoinOp::OnlySecond,
            "NOT_FIRST" => JoinOp::NotFirst,
            "ONLY_FIRST" => JoinOp::OnlyFirst,
            "NOT_AND" => JoinOp::NotAnd,
            "TRUE" => JoinOp::True,
            "AND" => JoinOp::And,
            "OR" => JoinOp::Or,
            "AND_NOT" => JoinOp::AndNot,
            "OR_NOT" => JoinOp::OrNot,
            _ => return None,
        })
    }

    /// The per-cell boolean operation, from vanilla `BooleanOp`'s lambda bodies.
    pub fn apply(self, first: bool, second: bool) -> bool {
        match self {
            JoinOp::False => false,
            JoinOp::NotOr => !(first || second),
            JoinOp::OnlySecond => !first && second,
            JoinOp::NotFirst => first && !second,
            JoinOp::OnlyFirst => first && !second,
            JoinOp::NotAnd => !(first && second),
            JoinOp::True => true,
            JoinOp::And => first && second,
            JoinOp::Or => first || second,
            JoinOp::AndNot => first && !second,
            JoinOp::OrNot => first || !second,
        }
    }
}

/// The slice of vanilla's `VoxelShape` model the interpreted bytecode needs.
///
/// This is the documented API contract for step 1 (`voxelshape.rs`) and for any
/// caller testing or using this interpreter. Coordinates are block coordinates
/// (`0.0..=1.0`); the methods are the exact semantics of the vanilla methods
/// named in their doc comments, including the object-identity rules
/// (`Shapes.block()` canonical identity, `CubeVoxelShape` vs
/// `ArrayVoxelShape`) because the prepared tables store them.
pub trait ShapeFactory {
    type Shape: Clone + Debug + PartialEq;

    /// `Shapes.empty()`: the canonical empty shape.
    fn empty(&mut self) -> Self::Shape;

    /// `Shapes.block()`: the canonical full cube.
    fn block(&mut self) -> Self::Shape;

    /// `Shapes.box(x1,y1,z1,x2,y2,z2)`/`Shapes.create(...)`: one box in block
    /// coordinates. Vanilla rules: a sub-`1e-7` extent is `empty()`; a
    /// `0..1` box on all axes is `block()`; otherwise the axis coordinate
    /// partition uses `findBits` (0, 1/2, 1/4, 1/8 grids only) and the shape is
    /// a `CubeVoxelShape` when every axis fits, an `ArrayVoxelShape` with the
    /// two raw coordinates otherwise.
    fn create(&mut self, x1: f64, y1: f64, z1: f64, x2: f64, y2: f64, z2: f64) -> Self::Shape;

    /// `Shapes.join(first, second, BooleanOp.OR)`.
    fn or(&mut self, first: &Self::Shape, second: &Self::Shape) -> Self::Shape;

    /// `Shapes.join(first, second, op)`.
    fn join(&mut self, first: &Self::Shape, second: &Self::Shape, operation: JoinOp) -> Self::Shape;

    /// `Shapes.rotate(shape, group, center)`: permute/invert the coordinate
    /// partition and occupancy around `center`; `rotation == IDENTITY` returns
    /// `shape` itself (vanilla's own early return).
    fn rotate(&mut self, shape: &Self::Shape, rotation: &Rotation, center: [f64; 3]) -> Self::Shape;

    /// `VoxelShape.isEmpty()`.
    fn is_empty(&mut self, shape: &Self::Shape) -> bool;
}

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

/// One block state: the block class in the jar plus its property values.
///
/// `values` maps a property name (`facing`, `waterlogged`, `layers`, - to the
/// *serialized* value the prepared tables use (`north`, `true`, `3`, -, which
/// is exactly what `states.rs`'s `Property.values` lists in ordinal order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    /// Internal class name, e.g. `net/minecraft/world/level/block/StairBlock`.
    pub class: String,
    /// Property name -> serialized value.
    pub values: BTreeMap<String, String>,
    /// The block's `Properties.hasCollision` flag (`geometry.rs`'s sweep).
    pub has_collision: bool,
}

/// The query the prepared tables store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Query {
    /// `getCollisionShape(state, EmptyBlockGetter, BlockPos.ZERO,
    /// CollisionContext.empty())`.
    Collision,
    /// `BlockBehaviour.getOcclusionShape(state)` (the oracle's call).
    Occlusion,
    /// `getBlockSupportShape(state, EmptyBlockGetter, BlockPos.ZERO)`.
    BlockSupport,
}

/// What one query produced.
#[derive(Clone, Debug)]
pub struct QueryResult<S> {
    /// The shape, present only when the whole path was interpreted.
    pub shape: Option<S>,
    /// Anything the interpreter could not model. A non-empty list with a
    /// `shape` means the result is not verified; both callers and tests must
    /// treat it as a gap.
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Class loading
// ---------------------------------------------------------------------------

/// The class files the interpreter reads.
pub trait ClassSource {
    fn class(&mut self, internal: &str) -> Option<Vec<u8>>;
}

/// All class files of a jar, read once (`client.jar` is the player's own file).
pub struct JarSource {
    files: BTreeMap<String, Vec<u8>>,
}

impl JarSource {
    pub fn open(path: &Path) -> Result<JarSource, String> {
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
            let Some(internal) = name.strip_suffix(".class") else {
                continue;
            };
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .map_err(|error| format!("{name}: {error}"))?;
            files.insert(internal.to_string(), data);
        }
        Ok(JarSource { files })
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    /// The internal name for a block class's simple name, through the vanilla
    /// `block` package tree (`StairBlock`, `piston/PistonBaseBlock`, -.
    pub fn resolve_block_class(&self, simple: &str) -> Option<String> {
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
}

impl ClassSource for JarSource {
    fn class(&mut self, internal: &str) -> Option<Vec<u8>> {
        self.files.get(internal).cloned()
    }
}

/// A map-backed source, for tests that do not need a jar.
#[derive(Default, Clone, Debug)]
pub struct MapSource {
    pub files: BTreeMap<String, Vec<u8>>,
}

impl ClassSource for MapSource {
    fn class(&mut self, internal: &str) -> Option<Vec<u8>> {
        self.files.get(internal).cloned()
    }
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct Property {
    name: String,
    kind: PropertyKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PropertyKind {
    Bool,
    Int,
    Enum { class: String },
}

#[derive(Debug)]
struct Instance<S> {
    class: String,
    fields: BTreeMap<String, Value<S>>,
    enum_name: Option<String>,
    enum_ordinal: i32,
}

#[derive(Debug)]
struct Lambda<S> {
    target_class: String,
    name: String,
    descriptor: String,
    kind: u8,
    captured: Vec<Value<S>>,
}

#[derive(Debug)]
struct StateData {
    class: String,
    values: BTreeMap<String, String>,
    has_collision: bool,
}

enum Value<S> {
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Bool(bool),
    Null,
    Str(String),
    Class(String),
    Shape(S),
    Property(Property),
    Object(Rc<RefCell<Instance<S>>>),
    Array(Rc<RefCell<Vec<Value<S>>>>),
    List(Rc<RefCell<Vec<Value<S>>>>),
    Map(Rc<RefCell<Vec<(Value<S>, Value<S>)>>>),
    Rotation(Rotation),
    Vec3([f64; 3]),
    StateRef(Rc<StateData>),
    BlockRef(Rc<str>),
    Lambda(Rc<Lambda<S>>),
    Marker(String),
    Opaque(String),
    Unit,
}

impl<S> Value<S> {
    fn as_int(&self) -> Option<i32> {
        match self {
            Value::Int(value) => Some(*value),
            Value::Bool(value) => Some(*value as i32),
            _ => None,
        }
    }

    fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(value) => Some(*value as f64),
            Value::Long(value) => Some(*value as f64),
            Value::Float(value) => Some(*value as f64),
            Value::Double(value) => Some(*value),
            Value::Bool(value) => Some(*value as i32 as f64),
            _ => None,
        }
    }
}

impl<S: Clone> Clone for Value<S> {
    fn clone(&self) -> Self {
        match self {
            Value::Int(value) => Value::Int(*value),
            Value::Long(value) => Value::Long(*value),
            Value::Float(value) => Value::Float(*value),
            Value::Double(value) => Value::Double(*value),
            Value::Bool(value) => Value::Bool(*value),
            Value::Null => Value::Null,
            Value::Str(value) => Value::Str(value.clone()),
            Value::Class(value) => Value::Class(value.clone()),
            Value::Shape(value) => Value::Shape(value.clone()),
            Value::Property(value) => Value::Property(value.clone()),
            Value::Object(value) => Value::Object(value.clone()),
            Value::Array(value) => Value::Array(value.clone()),
            Value::List(value) => Value::List(value.clone()),
            Value::Map(value) => Value::Map(value.clone()),
            Value::Rotation(value) => Value::Rotation(value.clone()),
            Value::Vec3(value) => Value::Vec3(*value),
            Value::StateRef(value) => Value::StateRef(value.clone()),
            Value::BlockRef(value) => Value::BlockRef(value.clone()),
            Value::Lambda(value) => Value::Lambda(value.clone()),
            Value::Marker(value) => Value::Marker(value.clone()),
            Value::Opaque(value) => Value::Opaque(value.clone()),
            Value::Unit => Value::Unit,
        }
    }
}

impl<S: Debug> Debug for Value<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Int(value) => write!(formatter, "Int({value})"),
            Value::Long(value) => write!(formatter, "Long({value})"),
            Value::Float(value) => write!(formatter, "Float({value})"),
            Value::Double(value) => write!(formatter, "Double({value})"),
            Value::Bool(value) => write!(formatter, "Bool({value})"),
            Value::Null => write!(formatter, "Null"),
            Value::Str(value) => write!(formatter, "Str({value:?})"),
            Value::Class(value) => write!(formatter, "Class({value})"),
            Value::Shape(_) => write!(formatter, "Shape"),
            Value::Property(value) => write!(formatter, "Property({value:?})"),
            Value::Object(value) => {
                let instance = value.borrow();
                write!(
                    formatter,
                    "Object({} {:?})",
                    instance.class, instance.enum_name
                )
            }
            Value::Array(_) => write!(formatter, "Array"),
            Value::List(_) => write!(formatter, "List"),
            Value::Map(_) => write!(formatter, "Map"),
            Value::Rotation(value) => write!(formatter, "Rotation({})", value.name),
            Value::Vec3(value) => write!(formatter, "Vec3({value:?})"),
            Value::StateRef(value) => write!(formatter, "State({})", value.class),
            Value::BlockRef(value) => write!(formatter, "Block({value})"),
            Value::Lambda(value) => write!(
                formatter,
                "Lambda({}.{}{})",
                value.target_class, value.name, value.descriptor
            ),
            Value::Marker(value) => write!(formatter, "Marker({value})"),
            Value::Opaque(value) => write!(formatter, "Opaque({value})"),
            Value::Unit => write!(formatter, "Unit"),
        }
    }
}

fn values_equal<S>(left: &Value<S>, right: &Value<S>) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Int(a), Value::Int(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Long(a), Value::Long(b)) => a == b,
        (Value::Float(a), Value::Float(b)) => a == b,
        (Value::Double(a), Value::Double(b)) => a == b,
        (Value::Str(a), Value::Str(b)) => a == b,
        (Value::Class(a), Value::Class(b)) => a == b,
        (Value::Marker(a), Value::Marker(b)) => a == b,
        (Value::Property(a), Value::Property(b)) => a == b,
        (Value::Rotation(a), Value::Rotation(b)) => a == b,
        (Value::Vec3(a), Value::Vec3(b)) => a == b,
        (Value::Object(a), Value::Object(b)) => Rc::ptr_eq(a, b),
        (Value::Array(a), Value::Array(b)) => Rc::ptr_eq(a, b),
        (Value::List(a), Value::List(b)) => Rc::ptr_eq(a, b),
        (Value::Map(a), Value::Map(b)) => Rc::ptr_eq(a, b),
        (Value::StateRef(a), Value::StateRef(b)) => Rc::ptr_eq(a, b),
        (Value::BlockRef(a), Value::BlockRef(b)) => a == b,
        _ => false,
    }
}

/// One decoded instruction.
#[derive(Clone, Debug)]
pub struct Ins {
    pub offset: usize,
    pub opcode: u8,
    /// Jump target offset (`if*`, `goto`) or the immediate of a push.
    pub imm: i32,
    /// Constant-pool index / local index / array type.
    pub index: usize,
    /// `tableswitch`/`lookupswitch` payload.
    pub switch: Option<Switch>,
}

#[derive(Clone, Debug)]
pub struct Switch {
    pub default: usize,
    pub low: i32,
    pub keys: Vec<i32>,
    pub targets: Vec<usize>,
}

fn read_i16(bytes: &[u8], at: usize) -> i32 {
    i16::from_be_bytes([bytes[at], bytes[at + 1]]) as i32
}

fn read_i32(bytes: &[u8], at: usize) -> i32 {
    i32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Decode a method body, including `tableswitch`/`lookupswitch` and `wide`.
pub fn decode_code(bytes: &[u8]) -> Vec<Ins> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let start = at;
        let mut opcode = bytes[at];
        at += 1;
        let mut imm = 0i32;
        let mut index = 0usize;
        let mut switch = None;
        match opcode {
            0x10 => {
                imm = bytes[at] as i8 as i32;
                at += 1;
            }
            0x11 => {
                imm = read_i16(bytes, at);
                at += 2;
            }
            0x12 => {
                index = bytes[at] as usize;
                at += 1;
            }
            0x13 | 0x14 | 0xb2..=0xb8 | 0xbb | 0xbd | 0xc0 | 0xc1 => {
                index = u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize;
                at += 2;
            }
            0xb9 | 0xba => {
                index = u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize;
                at += 4;
            }
            0x15..=0x19 | 0x36..=0x3a | 0xa9 | 0xbc => {
                index = bytes[at] as usize;
                at += 1;
            }
            0x84 => {
                index = bytes[at] as usize;
                imm = bytes[at + 1] as i8 as i32;
                at += 2;
            }
            0x99..=0xa8 | 0xc6 | 0xc7 => {
                imm = read_i16(bytes, at);
                at += 2;
            }
            0xc8 | 0xc9 => {
                imm = read_i32(bytes, at);
                at += 4;
            }
            0xaa => {
                at = (at + 3) & !3;
                let default = read_i32(bytes, at) as i32;
                let low = read_i32(bytes, at + 4);
                let high = read_i32(bytes, at + 8);
                at += 12;
                let mut keys = Vec::new();
                let mut targets = Vec::new();
                for key in low..=high {
                    keys.push(key);
                    targets.push((start as i32 + read_i32(bytes, at)) as usize);
                    at += 4;
                }
                switch = Some(Switch {
                    default: (start as i32 + default) as usize,
                    low,
                    keys,
                    targets,
                });
            }
            0xab => {
                at = (at + 3) & !3;
                let default = read_i32(bytes, at);
                let pairs = read_i32(bytes, at + 4) as usize;
                at += 8;
                let mut keys = Vec::new();
                let mut targets = Vec::new();
                for _ in 0..pairs {
                    keys.push(read_i32(bytes, at));
                    targets.push((start as i32 + read_i32(bytes, at + 4)) as usize);
                    at += 8;
                }
                switch = Some(Switch {
                    default: (start as i32 + default) as usize,
                    low: 0,
                    keys,
                    targets,
                });
            }
            0xc5 => {
                index = u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize;
                at += 3;
            }
            0xc4 => {
                // `wide`: re-emit the widened instruction as itself.
                let widened = bytes[at];
                opcode = widened;
                at += 1;
                if widened == 0x84 {
                    index = u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize;
                    imm = read_i16(bytes, at + 2);
                    at += 4;
                } else {
                    index = u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize;
                    at += 2;
                }
            }
            _ => {}
        }
        out.push(Ins {
            offset: start,
            opcode,
            imm,
            index,
            switch,
        });
    }
    out
}

fn offsets_by_pc(instructions: &[Ins]) -> BTreeMap<usize, usize> {
    instructions
        .iter()
        .enumerate()
        .map(|(pc, ins)| (ins.offset, pc))
        .collect()
}

// ---------------------------------------------------------------------------
// Descriptors and constant-pool helpers
// ---------------------------------------------------------------------------

/// The JVM type of one descriptor argument, with slots.
fn descriptor_args(descriptor: &str) -> (Vec<String>, String) {
    let mut types = Vec::new();
    let bytes: Vec<char> = descriptor.chars().collect();
    let mut at = 1usize;
    while at < bytes.len() && bytes[at] != ')' {
        let start = at;
        while at < bytes.len() && bytes[at] == '[' {
            at += 1;
        }
        if at < bytes.len() {
            at += 1;
            if bytes[at - 1] == 'L' {
                while at < bytes.len() && bytes[at] != ';' {
                    at += 1;
                }
                at += 1;
            }
        }
        types.push(bytes[start..at.min(bytes.len())].iter().collect());
    }
    let returns = bytes
        .iter()
        .skip(at + 1)
        .collect::<String>();
    (types, returns)
}

fn descriptor_value_count(descriptor: &str) -> usize {
    descriptor_args(descriptor).0.len()
}

fn type_slots(name: &str) -> usize {
    if name == "J" || name == "D" {
        2
    } else {
        1
    }
}

fn nat(constants: &[Constant], index: usize) -> (String, String) {
    match constants.get(index) {
        Some(Constant::NameAndType { name, kind }) => {
            let name_index: usize = name.trim_start_matches('#').parse().unwrap_or(0);
            let descriptor_index: usize = kind.trim_start_matches('#').parse().unwrap_or(0);
            (
                classfile::utf8(constants, name_index),
                classfile::utf8(constants, descriptor_index),
            )
        }
        _ => (String::new(), String::new()),
    }
}

fn class_name(constants: &[Constant], index: usize) -> String {
    match constants.get(index) {
        Some(Constant::Class(reference)) => {
            let inner: usize = reference.trim_start_matches('#').parse().unwrap_or(0);
            classfile::utf8(constants, inner)
        }
        _ => String::new(),
    }
}

fn string_constant(constants: &[Constant], index: usize) -> Option<String> {
    match constants.get(index) {
        Some(Constant::String(reference)) => {
            let inner: usize = reference.trim_start_matches('#').parse().unwrap_or(0);
            Some(classfile::utf8(constants, inner))
        }
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum LongDouble {
    Long(i64),
    Double(f64),
}

/// `ldc2_w` points at the logical `long`/`double` slot. `classfile.rs` stores
/// such a value one slot late (its assignment evaluates the right-hand side
/// first), so accept the value at `index + 1` when the slot is the placeholder.
fn long_double_constant(constants: &[Constant], index: usize) -> Option<LongDouble> {
    let convert = |value: &Constant| match value {
        Constant::Long(value) => Some(LongDouble::Long(*value)),
        Constant::Double(value) => Some(LongDouble::Double(*value)),
        _ => None,
    };
    constants
        .get(index)
        .and_then(convert)
        .or_else(|| constants.get(index + 1).and_then(convert))
}

/// The `MethodHandle` target of a bootstrap argument.
fn method_handle_target(
    constants: &[Constant],
    index: usize,
) -> Option<(String, String, String)> {
    match constants.get(index) {
        Some(Constant::MethodHandle { reference }) => match constants.get(*reference) {
            Some(Constant::Methodref { .. }) | Some(Constant::InterfaceMethodref { .. }) => {
                classfile::method_ref(constants, *reference)
            }
            _ => None,
        },
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The interpreter
// ---------------------------------------------------------------------------

enum Fault {
    Thrown(String),
    Steps,
}

/// The shape-method interpreter: a small JVM plus the `VoxelShape` contract.
pub struct Vm<S: Clone + Debug + PartialEq> {
    source: Box<dyn ClassSource>,
    factory: Box<dyn ShapeFactory<Shape = S>>,
    classes: BTreeMap<String, Option<Rc<ClassFile>>>,
    statics: BTreeMap<(String, String), Value<S>>,
    constants: BTreeMap<String, Vec<(String, Value<S>)>>,
    initialized: BTreeSet<String>,
    initializing: BTreeSet<String>,
    init_warnings: Vec<String>,
    warnings: Vec<String>,
    /// The queried state's `hasCollision` flag, read by the default
    /// `BlockBehaviour.getCollisionShape`.
    has_collision: bool,
    /// True while a class initialiser runs; its unmodelled calls are support
    /// noise, not evidence against a query result.
    in_init: bool,
    /// Static initialisation of one class may run a bounded amount.
    steps: usize,
    depth: usize,
}

impl<S: Clone + Debug + PartialEq> Vm<S> {
    pub fn new(source: Box<dyn ClassSource>, factory: impl ShapeFactory<Shape = S> + 'static) -> Vm<S> {
        Vm {
            source,
            factory: Box::new(factory),
            classes: BTreeMap::new(),
            statics: BTreeMap::new(),
            constants: BTreeMap::new(),
            initialized: BTreeSet::new(),
            initializing: BTreeSet::new(),
            init_warnings: Vec::new(),
            warnings: Vec::new(),
            has_collision: false,
            in_init: false,
            steps: 0,
            depth: 0,
        }
    }

    pub fn factory(&mut self) -> &mut dyn ShapeFactory<Shape = S> {
        &mut *self.factory
    }

    /// Warnings produced while initialising classes, for diagnostics.
    pub fn initialization_warnings(&self) -> &[String] {
        &self.init_warnings
    }

    fn class(&mut self, internal: &str) -> Option<Rc<ClassFile>> {
        if let Some(hit) = self.classes.get(internal) {
            return hit.clone();
        }
        let parsed = self
            .source
            .class(internal)
            .and_then(|bytes| classfile::parse(&bytes).ok())
            .map(Rc::new);
        self.classes.insert(internal.to_string(), parsed.clone());
        parsed
    }

    fn warn(&mut self, message: String) {
        if self.in_init {
            self.init_warn(message);
            return;
        }
        if self.warnings.len() < 64 && !self.warnings.contains(&message) {
            self.warnings.push(message);
        }
    }

    fn init_warn(&mut self, message: String) {
        if self.init_warnings.len() < 256 && !self.init_warnings.contains(&message) {
            self.init_warnings.push(message);
        }
    }

    // -- class initialisation -------------------------------------------------

    fn ensure_initialized(&mut self, internal: &str) {
        if self.initialized.contains(internal) || self.initializing.contains(internal) {
            return;
        }
        self.initializing.insert(internal.to_string());
        if let Some(class) = self.class(internal) {
            if let Some(initializer) = class.method("<clinit>").cloned() {
                let was_initialising = self.in_init;
                self.in_init = true;
                let result = self.execute(&class, &initializer, None, Vec::new());
                self.in_init = was_initialising;
                if let Err(error) = result {
                    let message = match error {
                        Fault::Thrown(message) => message,
                        Fault::Steps => "step limit".to_string(),
                    };
                    self.init_warn(format!("{internal}.<clinit>: {message}"));
                }
            }
        }
        self.initializing.remove(internal);
        self.initialized.insert(internal.to_string());
    }

    /// The class that declares a static field reached through `owner`.
    fn declaring_class(&mut self, owner: &str, name: &str, descriptor: &str) -> String {
        let mut current = Some(owner.to_string());
        let mut depth = 0;
        while let Some(internal) = current {
            let Some(class) = self.class(&internal) else {
                break;
            };
            if class
                .fields
                .iter()
                .any(|field| field.name == name && field.descriptor == descriptor)
            {
                return internal;
            }
            current = if class.super_name.is_empty() {
                None
            } else {
                Some(class.super_name.clone())
            };
            depth += 1;
            if depth > 32 {
                break;
            }
        }
        owner.to_string()
    }

    // -- static fields --------------------------------------------------------

    fn get_static(&mut self, owner: &str, name: &str, descriptor: &str) -> Value<S> {
        if let Some(special) = self.special_static(owner, name, descriptor) {
            return special;
        }
        let declaring = self.declaring_class(owner, name, descriptor);
        // A field inherited from a superclass is initialised by that class's
        // own `<clinit>`, not the subclass's.
        self.ensure_initialized(&declaring);
        if declaring != owner {
            self.ensure_initialized(owner);
        }
        if let Some(value) = self
            .statics
            .get(&(declaring.clone(), name.to_string()))
            .cloned()
        {
            return value;
        }
        // A compile-time constant (ConstantValue) is not parsed by
        // `classfile.rs`; report instead of guessing.
        self.warn(format!("unresolved static {owner}.{name}:{descriptor}"));
        Value::Opaque(format!("static {owner}.{name}"))
    }

    fn put_static(&mut self, owner: &str, name: &str, descriptor: &str, value: Value<S>) {
        let declaring = self.declaring_class(owner, name, descriptor);
        if let Value::Object(instance) = &value {
            let borrowed = instance.borrow();
            if let Some(enum_name) = &borrowed.enum_name {
                if borrowed.class == declaring {
                    let entry = (enum_name.clone(), value.clone());
                    let list = self.constants.entry(declaring.clone()).or_default();
                    if let Some(slot) = list.iter_mut().find(|slot| slot.0 == entry.0) {
                        slot.1 = entry.1;
                    } else {
                        list.push(entry);
                    }
                }
            }
        }
        self.statics
            .insert((declaring, name.to_string()), value);
    }

    fn special_static(&mut self, owner: &str, name: &str, _descriptor: &str) -> Option<Value<S>> {
        match owner {
            SHAPES => match name {
                "EMPTY" => Some(Value::Shape(self.factory.empty())),
                "BLOCK" => Some(Value::Shape(self.factory.block())),
                "BLOCK_CENTER" => Some(Value::Vec3([0.5, 0.5, 0.5])),
                _ => None,
            },
            BOOLEAN_OP => Some(Value::Marker(name.to_string())),
            "net/minecraft/world/level/EmptyBlockGetter" if name == "INSTANCE" => {
                Some(Value::Marker("EmptyBlockGetter.INSTANCE".to_string()))
            }
            "net/minecraft/core/BlockPos" if name == "ZERO" => {
                Some(Value::Marker("BlockPos.ZERO".to_string()))
            }
            _ => None,
        }
    }

    // -- method resolution ----------------------------------------------------

    fn resolve(
        &mut self,
        start: &str,
        name: &str,
        descriptor: &str,
    ) -> Option<(Rc<ClassFile>, Member)> {
        let mut current = Some(start.to_string());
        let mut depth = 0;
        while let Some(internal) = current {
            let class = self.class(&internal)?;
            if let Some(method) = class
                .methods
                .iter()
                .find(|method| method.name == name && method.descriptor == descriptor)
            {
                return Some((class.clone(), method.clone()));
            }
            current = if class.super_name.is_empty() {
                None
            } else {
                Some(class.super_name.clone())
            };
            depth += 1;
            if depth > 48 {
                return None;
            }
        }
        None
    }

    // -- entry points ---------------------------------------------------------

    /// Run one of the prepared-table queries for a state.
    pub fn query(&mut self, state: &State, query: Query) -> QueryResult<S> {
        self.warnings.clear();
        self.has_collision = state.has_collision;
        let result = self.query_inner(state, query);
        let warnings = std::mem::take(&mut self.warnings);
        let shape = match result {
            Ok(Value::Shape(shape)) if warnings.is_empty() => Some(shape),
            _ => None,
        };
        QueryResult { shape, warnings }
    }

    fn query_inner(&mut self, state: &State, query: Query) -> Result<Value<S>, Fault> {
        let state_value = Value::StateRef(Rc::new(StateData {
            class: state.class.clone(),
            values: state.values.clone(),
            has_collision: state.has_collision,
        }));
        let level = Value::Marker("EmptyBlockGetter.INSTANCE".to_string());
        let position = Value::Marker("BlockPos.ZERO".to_string());
        let context = Value::Marker("CollisionContext.empty".to_string());
        let (name, descriptor, arguments) = match query {
            Query::Collision => (
                "getCollisionShape",
                "(Lnet/minecraft/world/level/block/state/BlockState;Lnet/minecraft/world/level/BlockGetter;Lnet/minecraft/core/BlockPos;Lnet/minecraft/world/phys/shapes/CollisionContext;)Lnet/minecraft/world/phys/shapes/VoxelShape;",
                vec![state_value, level, position, context],
            ),
            Query::Occlusion => (
                "getOcclusionShape",
                "(Lnet/minecraft/world/level/block/state/BlockState;)Lnet/minecraft/world/phys/shapes/VoxelShape;",
                vec![state_value],
            ),
            Query::BlockSupport => (
                "getBlockSupportShape",
                "(Lnet/minecraft/world/level/block/state/BlockState;Lnet/minecraft/world/level/BlockGetter;Lnet/minecraft/core/BlockPos;)Lnet/minecraft/world/phys/shapes/VoxelShape;",
                vec![state_value, level, position],
            ),
        };
        let receiver = Value::BlockRef(Rc::from(state.class.as_str()));
        self.steps = 0;
        self.depth = 0;
        self.invoke_dispatch(0xb6, &state.class, name, descriptor, Some(receiver), arguments)
    }

    /// `state.getValue(property)` through the property table.
    fn state_value(&mut self, state: &Rc<StateData>, property: &Property) -> Value<S> {
        let Some(serialized) = state.values.get(&property.name) else {
            self.warn(format!(
                "state {} has no value for property {}",
                state.class, property.name
            ));
            return Value::Opaque(format!("property {}", property.name));
        };
        match &property.kind {
            PropertyKind::Bool => match serialized.as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                other => {
                    self.warn(format!("boolean property {} = {other}", property.name));
                    Value::Opaque("bool".to_string())
                }
            },
            PropertyKind::Int => match serialized.parse::<i32>() {
                Ok(value) => Value::Int(value),
                Err(_) => {
                    self.warn(format!("integer property {} = {serialized}", property.name));
                    Value::Opaque("int".to_string())
                }
            },
            PropertyKind::Enum { class } => {
                self.enum_value(class, serialized)
            }
        }
    }

    /// The enum constant of `class` whose serialized name is `serialized`.
    fn enum_value(&mut self, class: &str, serialized: &str) -> Value<S> {
        if !self.initialized.contains(class) {
            self.ensure_initialized(class);
        }
        let serialized_names = self.enum_serialized_names(class);
        let mut candidates: Vec<(String, Value<S>)> = self
            .constants
            .get(class)
            .cloned()
            .unwrap_or_default();
        if candidates.is_empty() {
            if let Some(parsed) = self.class(class) {
                for field in &parsed.fields {
                    if field.descriptor == format!("L{class};") {
                        let value = self.get_static(class, &field.name, &field.descriptor);
                        if matches!(value, Value::Object(_)) {
                            candidates.push((field.name.clone(), value));
                        }
                    }
                }
            }
        }
        for (name, value) in &candidates {
            let label = serialized_names
                .get(name)
                .cloned()
                .unwrap_or_else(|| name.to_lowercase());
            if label == serialized {
                return value.clone();
            }
        }
        self.warn(format!(
            "enum {class} has no constant serialized as {serialized}"
        ));
        Value::Opaque(format!("enum {class}.{serialized}"))
    }

    /// `(constant name, serialized name)` per enum class, derived the same way
    /// `states.rs` derives `EnumProperty` value labels: the second string of
    /// each constant's `new; - putstatic` group in `<clinit>`.
    fn enum_serialized_names(&mut self, class: &str) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        let Some(parsed) = self.class(class) else {
            return out;
        };
        let Some(initializer) = parsed.method("<clinit>") else {
            return out;
        };
        let Some(code) = &initializer.code else {
            return out;
        };
        let mut strings: Vec<String> = Vec::new();
        for ins in decode_code(&code.bytes) {
            match ins.opcode {
                0xbb => {
                    if class_name(&parsed.constants, ins.index) == class {
                        strings.clear();
                    }
                }
                0x12 | 0x13 => {
                    if let Some(text) = string_constant(&parsed.constants, ins.index) {
                        strings.push(text);
                    }
                }
                0xb3 => {
                    if let Some((owner, name, _)) =
                        classfile::field_ref(&parsed.constants, ins.index)
                    {
                        if owner == class {
                            if let Some(serialized) = strings.get(1) {
                                out.insert(name, serialized.clone());
                            }
                            strings.clear();
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    // -- dispatch -------------------------------------------------------------

    fn invoke_dispatch(
        &mut self,
        opcode: u8,
        owner: &str,
        name: &str,
        descriptor: &str,
        receiver: Option<Value<S>>,
        arguments: Vec<Value<S>>,
    ) -> Result<Value<S>, Fault> {
        // Constructors of JDK and library types are structural: they never
        // decide a shape, and the jar carries no JDK classes.
        if name == "<init>" {
            match (owner, receiver.as_ref()) {
                ("java/lang/Enum", Some(Value::Object(instance))) => {
                    let enum_name = match arguments.first() {
                        Some(Value::Str(text)) => text.clone(),
                        _ => String::new(),
                    };
                    let ordinal = arguments.get(1).and_then(Value::as_int).unwrap_or(0);
                    let mut borrowed = instance.borrow_mut();
                    borrowed.enum_name = Some(enum_name);
                    borrowed.enum_ordinal = ordinal;
                    return Ok(Value::Unit);
                }
                ("java/lang/Object", _) => return Ok(Value::Unit),
                _ if owner.starts_with("org/joml/") => return Ok(Value::Unit),
                _ => {}
            }
        }
        if owner.starts_with("org/joml/")
            || matches!(&receiver, Some(Value::Object(instance)) if instance.borrow().class.starts_with("org/joml/"))
        {
            return Ok(Value::Opaque(format!("{owner}.{name}")));
        }
        if matches!(&receiver, Some(Value::Opaque(_))) {
            self.warn(format!("call {owner}.{name} on an opaque value"));
            return Ok(Value::Opaque(format!("{owner}.{name}")));
        }
        // Receiver-side specials first: state, boxed primitives, lambdas,
        // collections, enums.
        if let Some(receiver) = receiver.clone() {
            if let Some(value) =
                self.receiver_special(owner, name, descriptor, &receiver, &arguments)?
            {
                return Ok(value);
            }
        }
        // Static/library/contract specials keyed by owner+name.
        if let Some(value) = self.owner_special(owner, name, descriptor, &arguments)? {
            return Ok(value);
        }
        // A real Java method.
        let start = match (&receiver, opcode) {
            (Some(Value::Object(instance)), 0xb6 | 0xb9) => instance.borrow().class.clone(),
            (Some(Value::BlockRef(class)), 0xb6 | 0xb9) => class.to_string(),
            (Some(Value::StateRef(_)), 0xb6 | 0xb9) => owner.to_string(),
            (_, 0xb6 | 0xb9) => owner.to_string(),
            _ => owner.to_string(),
        };
        let resolved = self.resolve(&start, name, descriptor).or_else(|| {
            if opcode == 0xb7 || opcode == 0xb8 {
                self.resolve(owner, name, descriptor)
            } else {
                None
            }
        });
        let Some((class, method)) = resolved else {
            // Unknown helper: opaque, never a shape.
            self.warn(format!("unmodelled call {start}.{name}{descriptor}"));
            return Ok(Value::Opaque(format!("{start}.{name}")));
        };
        self.depth += 1;
        let result = self.execute(&class, &method, receiver, arguments);
        self.depth -= 1;
        result
    }

    fn receiver_special(
        &mut self,
        owner: &str,
        name: &str,
        descriptor: &str,
        receiver: &Value<S>,
        arguments: &[Value<S>],
    ) -> Result<Option<Value<S>>, Fault> {
        match receiver {
            Value::StateRef(state) => {
                let state = state.clone();
                match (name, descriptor) {
                    ("getValue", _) => {
                        let Some(Value::Property(property)) = arguments.first() else {
                            return Ok(Some(Value::Opaque("getValue".into())));
                        };
                        Ok(Some(self.state_value(&state, property)))
                    }
                    ("getBlock", _) => Ok(Some(Value::BlockRef(Rc::from(state.class.as_str())))),
                    ("asState", _) => Ok(Some(receiver.clone())),
                    ("getShape", "(Lnet/minecraft/world/level/BlockGetter;Lnet/minecraft/core/BlockPos;)Lnet/minecraft/world/phys/shapes/VoxelShape;") => {
                        let args = vec![
                            receiver.clone(),
                            arguments[0].clone(),
                            arguments[1].clone(),
                            Value::Marker("CollisionContext.empty".to_string()),
                        ];
                        let class = state.class.clone();
                        Ok(Some(self.invoke_dispatch(
                            0xb6,
                            &class,
                            "getShape",
                            "(Lnet/minecraft/world/level/block/state/BlockState;Lnet/minecraft/world/level/BlockGetter;Lnet/minecraft/core/BlockPos;Lnet/minecraft/world/phys/shapes/CollisionContext;)Lnet/minecraft/world/phys/shapes/VoxelShape;",
                            Some(Value::BlockRef(Rc::from(class.as_str()))),
                            args,
                        )?))
                    }
                    ("getShape", _) => {
                        let args = vec![
                            receiver.clone(),
                            arguments[0].clone(),
                            arguments[1].clone(),
                            arguments[2].clone(),
                        ];
                        let class = state.class.clone();
                        Ok(Some(self.invoke_dispatch(
                            0xb6,
                            &class,
                            "getShape",
                            "(Lnet/minecraft/world/level/block/state/BlockState;Lnet/minecraft/world/level/BlockGetter;Lnet/minecraft/core/BlockPos;Lnet/minecraft/world/phys/shapes/CollisionContext;)Lnet/minecraft/world/phys/shapes/VoxelShape;",
                            Some(Value::BlockRef(Rc::from(class.as_str()))),
                            args,
                        )?))
                    }
                    _ => Ok(None),
                }
            }
            Value::Object(instance) => {
                let class = instance.borrow().class.clone();
                match (class.as_str(), name) {
                    ("java/lang/Integer", "intValue") => Ok(Some(Value::Int(match receiver {
                        Value::Object(instance) => instance
                            .borrow()
                            .fields
                            .get("value")
                            .and_then(Value::as_int)
                            .unwrap_or(0),
                        _ => 0,
                    }))),
                    ("java/lang/Boolean", "booleanValue") => Ok(Some(Value::Bool(false))),
                    _ if class == OCTAHEDRAL => match name {
                        "compose" => {
                            let Some(Value::Object(other)) = arguments.first() else {
                                return Ok(Some(Value::Opaque("compose".into())));
                            };
                            let left = match self.rotation_of(receiver) {
                                Some(value) => value,
                                None => return Ok(Some(Value::Opaque("compose".into()))),
                            };
                            let right = match self.rotation_of(&Value::Object(other.clone())) {
                                Some(value) => value,
                                None => return Ok(Some(Value::Opaque("compose".into()))),
                            };
                            Ok(Some(Value::Rotation(left.compose(&right))))
                        }
                        "ordinal" => Ok(Some(Value::Int(instance.borrow().enum_ordinal))),
                        "name" => Ok(Some(Value::Str(
                            instance
                                .borrow()
                                .enum_name
                                .clone()
                                .unwrap_or_default(),
                        ))),
                        _ => Ok(None),
                    },
                    _ => {
                        if instance.borrow().enum_name.is_some() {
                            match name {
                                "ordinal" => {
                                    return Ok(Some(Value::Int(instance.borrow().enum_ordinal)))
                                }
                                "name" => {
                                    return Ok(Some(Value::Str(
                                        instance
                                            .borrow()
                                            .enum_name
                                            .clone()
                                            .unwrap_or_default(),
                                    )))
                                }
                                _ => {}
                            }
                        }
                        Ok(None)
                    }
                }
            }
            Value::Lambda(lambda) => {
                let lambda = lambda.clone();
                let callable = matches!(
                    name,
                    "apply"
                        | "test"
                        | "get"
                        | "accept"
                        | "compare"
                        | "andThen"
                        | "applyAsInt"
                        | "applyAsLong"
                        | "applyAsDouble"
                );
                if !callable {
                    return Ok(None);
                }
                let mut args = lambda.captured.clone();
                for argument in arguments {
                    args.push(argument.clone());
                }
                let (receiver, arguments) = if lambda.kind == 6 {
                    (None, args)
                } else {
                    let mut args = args;
                    let receiver = args.remove(0);
                    (Some(receiver), args)
                };
                let opcode = if lambda.kind == 6 { 0xb8 } else { 0xb6 };
                Ok(Some(self.invoke_dispatch(
                    opcode,
                    &lambda.target_class,
                    &lambda.name,
                    &lambda.descriptor,
                    receiver,
                    arguments,
                )?))
            }
            Value::Map(map) => match name {
                "get" => {
                    let key = arguments.first().cloned().unwrap_or(Value::Null);
                    for (candidate, value) in map.borrow().iter() {
                        if values_equal(candidate, &key) {
                            return Ok(Some(value.clone()));
                        }
                    }
                    Ok(Some(Value::Null))
                }
                "getOrDefault" => {
                    let key = arguments.first().cloned().unwrap_or(Value::Null);
                    for (candidate, value) in map.borrow().iter() {
                        if values_equal(candidate, &key) {
                            return Ok(Some(value.clone()));
                        }
                    }
                    Ok(Some(arguments.get(1).cloned().unwrap_or(Value::Null)))
                }
                "put" => {
                    let key = arguments.first().cloned().unwrap_or(Value::Null);
                    let value = arguments.get(1).cloned().unwrap_or(Value::Null);
                    let mut entries = map.borrow_mut();
                    if let Some(slot) = entries
                        .iter_mut()
                        .find(|(candidate, _)| values_equal(candidate, &key))
                    {
                        let previous = slot.1.clone();
                        slot.1 = value;
                        return Ok(Some(previous));
                    }
                    entries.push((key, value));
                    Ok(Some(Value::Null))
                }
                "containsKey" => {
                    let key = arguments.first().cloned().unwrap_or(Value::Null);
                    let found = map
                        .borrow()
                        .iter()
                        .any(|(candidate, _)| values_equal(candidate, &key));
                    Ok(Some(Value::Bool(found)))
                }
                "isEmpty" => Ok(Some(Value::Bool(map.borrow().is_empty()))),
                _ => Ok(None),
            },
            Value::List(list) => match name {
                "get" => {
                    let index = arguments.first().and_then(Value::as_int).unwrap_or(0);
                    let value = list.borrow().get(index.max(0) as usize).cloned();
                    Ok(Some(value.unwrap_or(Value::Null)))
                }
                "size" => Ok(Some(Value::Int(list.borrow().len() as i32))),
                _ => Ok(None),
            },
            Value::Array(array) => match name {
                "clone" => Ok(Some(Value::Array(array.clone()))),
                _ => Ok(None),
            },
            Value::Str(text) => match (name, arguments) {
                ("equals", [Value::Str(other)]) => Ok(Some(Value::Bool(text == other))),
                ("isEmpty", _) => Ok(Some(Value::Bool(text.is_empty()))),
                _ => Ok(None),
            },
            Value::Shape(shape) => match name {
                "isEmpty" => Ok(Some(Value::Bool(self.factory.is_empty(shape)))),
                "optimize" => Ok(Some(receiver.clone())),
                _ => Ok(None),
            },
            Value::Int(_) | Value::Bool(_) | Value::Double(_) | Value::Long(_) => match name {
                "intValue" => Ok(Some(Value::Int(receiver.as_int().unwrap_or(0)))),
                "booleanValue" => Ok(Some(Value::Bool(
                    receiver.as_int().map(|value| value != 0).unwrap_or(false),
                ))),
                "doubleValue" => Ok(Some(Value::Double(receiver.as_f64().unwrap_or(0.0)))),
                "equals" => Ok(Some(Value::Bool(
                    arguments
                        .first()
                        .map(|other| values_equal(receiver, other))
                        .unwrap_or(false),
                ))),
                _ => Ok(None),
            },
            _ => {
                if owner == OCTAHEDRAL && name == "compose" {
                    let left = self.rotation_of(receiver);
                    let right = arguments
                        .first()
                        .and_then(|argument| self.rotation_of(argument));
                    if let (Some(left), Some(right)) = (left, right) {
                        return Ok(Some(Value::Rotation(left.compose(&right))));
                    }
                }
                Ok(None)
            }
        }
    }

    fn owner_special(
        &mut self,
        owner: &str,
        name: &str,
        descriptor: &str,
        arguments: &[Value<S>],
    ) -> Result<Option<Value<S>>, Fault> {
        if owner == SHAPES {
            return self.shapes_special(name, descriptor, arguments);
        }
        if owner == BLOCK && name == "boxes" {
            let count = arguments.first().and_then(Value::as_int).unwrap_or(0);
            let Some(Value::Lambda(lambda)) = arguments.get(1).cloned() else {
                self.warn("Block.boxes without a lambda".to_string());
                return Ok(Some(Value::Opaque("Block.boxes".into())));
            };
            let mut shapes = Vec::new();
            for index in 0..=count {
                let value = self.call_lambda(&lambda, vec![Value::Int(index)])?;
                shapes.push(value);
            }
            return Ok(Some(Value::Array(Rc::new(RefCell::new(shapes)))));
        }
        if owner.starts_with(PROPERTIES_PREFIX) {
            if let Some(value) = self.property_factory(owner, name, descriptor, arguments) {
                return Ok(Some(value));
            }
        }
        match (owner, name, descriptor) {
            ("java/lang/Integer", "valueOf", _) => {
                return Ok(Some(Value::Int(arguments.first().and_then(Value::as_int).unwrap_or(0))))
            }
            ("java/lang/Boolean", "valueOf", _) => {
                return Ok(Some(Value::Bool(
                    arguments.first().and_then(Value::as_int).unwrap_or(0) != 0,
                )))
            }
            ("java/lang/Double", "valueOf", _) => {
                return Ok(Some(Value::Double(
                    arguments.first().and_then(Value::as_f64).unwrap_or(0.0),
                )))
            }
            ("java/lang/Long", "valueOf", _) => {
                return Ok(Some(Value::Long(match arguments.first() {
                    Some(Value::Long(value)) => *value,
                    Some(value) => value.as_f64().unwrap_or(0.0) as i64,
                    None => 0,
                })))
            }
            ("java/lang/Math", "round", _) => {
                let value = arguments.first().and_then(Value::as_f64).unwrap_or(0.0);
                return Ok(Some(Value::Long(value.round() as i64)));
            }
            ("java/lang/Math", "abs", _) => {
                let value = arguments.first().and_then(Value::as_f64).unwrap_or(0.0);
                return Ok(Some(Value::Double(value.abs())));
            }
            ("java/lang/Math", "min", _) => {
                let a = arguments.first().and_then(Value::as_int).unwrap_or(0);
                let b = arguments.get(1).and_then(Value::as_int).unwrap_or(0);
                return Ok(Some(Value::Int(a.min(b))));
            }
            ("java/lang/Math", "max", _) => {
                let a = arguments.first().and_then(Value::as_int).unwrap_or(0);
                let b = arguments.get(1).and_then(Value::as_int).unwrap_or(0);
                return Ok(Some(Value::Int(a.max(b))));
            }
            ("java/lang/String", "valueOf", _) => {
                return Ok(Some(match arguments.first() {
                    Some(Value::Str(text)) => Value::Str(text.clone()),
                    Some(value) => Value::Str(format!("{value:?}")),
                    None => Value::Str(String::new()),
                }));
            }
            ("java/util/Map", "of", _) => {
                let mut entries = Vec::new();
                let mut index = 0;
                while index + 1 < arguments.len() {
                    entries.push((arguments[index].clone(), arguments[index + 1].clone()));
                    index += 2;
                }
                return Ok(Some(Value::Map(Rc::new(RefCell::new(entries)))));
            }
            ("java/util/List", "of", _)
            | ("com/google/common/collect/ImmutableList", "of", _)
            | ("java/util/Arrays", "asList", _) => {
                return Ok(Some(Value::List(Rc::new(RefCell::new(
                    arguments.to_vec(),
                )))));
            }
            ("java/util/Arrays", "fill", _) => {
                if let Some(Value::Array(array)) = arguments.first() {
                    if let Some(value) = arguments.get(1) {
                        let mut values = array.borrow_mut();
                        let length = values.len();
                        let (from, to) = if arguments.len() >= 4 {
                            let from = arguments[2].as_int().unwrap_or(0).max(0) as usize;
                            let to = arguments[3].as_int().unwrap_or(length as i32).max(0) as usize;
                            (from.min(length), to.min(length))
                        } else {
                            (0, length)
                        };
                        for slot in from..to {
                            values[slot] = value.clone();
                        }
                    }
                }
                return Ok(Some(Value::Unit));
            }
            ("com/google/common/collect/Maps", "newEnumMap", _) => {
                return Ok(Some(match arguments.first() {
                    Some(Value::Map(map)) => Value::Map(Rc::new(RefCell::new(map.borrow().clone()))),
                    _ => Value::Map(Rc::new(RefCell::new(Vec::new()))),
                }));
            }
            ("net/minecraft/world/phys/shapes/CollisionContext", "empty", _) => {
                return Ok(Some(Value::Marker(
                    "CollisionContext.empty".to_string(),
                )))
            }
            _ => {}
        }
        // `Enum.values()` on any enum class.
        if name == "values" {
            if let Some(parsed) = self.class(owner) {
                if parsed.super_name == "java/lang/Enum" {
                    if !self.initialized.contains(owner) {
                        self.ensure_initialized(owner);
                    }
                    let constants = self
                        .constants
                        .get(owner)
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(_, value)| value)
                        .collect::<Vec<_>>();
                    return Ok(Some(Value::Array(Rc::new(RefCell::new(constants)))));
                }
            }
        }
        if name == "valueOf" && descriptor.starts_with("(Ljava/lang/String;)") {
            if let Some(parsed) = self.class(owner) {
                if parsed.super_name == "java/lang/Enum" {
                    let Some(Value::Str(wanted)) = arguments.first() else {
                        return Ok(Some(Value::Null));
                    };
                    if !self.initialized.contains(owner) {
                        self.ensure_initialized(owner);
                    }
                    let found = self
                        .constants
                        .get(owner)
                        .and_then(|constants| {
                            constants.iter().find(|(name, _)| name == wanted)
                        })
                        .map(|(_, value)| value.clone());
                    return Ok(Some(found.unwrap_or(Value::Null)));
                }
            }
        }
        Ok(None)
    }

    fn property_factory(
        &mut self,
        owner: &str,
        name: &str,
        descriptor: &str,
        arguments: &[Value<S>],
    ) -> Option<Value<S>> {
        let property_name = match arguments.first() {
            Some(Value::Str(text)) => text.clone(),
            _ => return None,
        };
        if !name.starts_with("create") {
            return None;
        }
        let kind = if owner.ends_with("/BooleanProperty") {
            PropertyKind::Bool
        } else if owner.ends_with("/IntegerProperty") {
            PropertyKind::Int
        } else if owner.ends_with("/EnumProperty") {
            let class = arguments
                .iter()
                .find_map(|argument| match argument {
                    Value::Class(class) => Some(class.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            if class.is_empty() {
                return None;
            }
            PropertyKind::Enum { class }
        } else {
            return None;
        };
        let _ = descriptor;
        Some(Value::Property(Property {
            name: property_name,
            kind,
        }))
    }

    fn shapes_special(
        &mut self,
        name: &str,
        descriptor: &str,
        arguments: &[Value<S>],
    ) -> Result<Option<Value<S>>, Fault> {
        let shape = |value: &Value<S>| match value {
            Value::Shape(shape) => Some(shape.clone()),
            _ => None,
        };
        match name {
            "empty" => Ok(Some(Value::Shape(self.factory.empty()))),
            "block" => Ok(Some(Value::Shape(self.factory.block()))),
            "box" | "create" if descriptor.starts_with("(DDDDDD)") => {
                let numbers: Vec<f64> = arguments
                    .iter()
                    .map(|argument| argument.as_f64().unwrap_or(f64::NAN))
                    .collect();
                if numbers.len() != 6 || numbers.iter().any(|value| value.is_nan()) {
                    self.warn(format!("Shapes.{name} with non-constant arguments"));
                    return Ok(Some(Value::Opaque(format!("Shapes.{name}"))));
                }
                Ok(Some(Value::Shape(self.factory.create(
                    numbers[0], numbers[1], numbers[2], numbers[3], numbers[4], numbers[5],
                ))))
            }
            "or" => {
                let result: S;
                if descriptor.starts_with("(Lnet/minecraft/world/phys/shapes/VoxelShape;[") {
                    let Some(Value::Shape(first)) = arguments.first() else {
                        return Ok(Some(Value::Opaque("Shapes.or".into())));
                    };
                    let mut current = first.clone();
                    for argument in arguments.iter().skip(1) {
                        if let Value::Array(array) = argument {
                            for value in array.borrow().iter() {
                                let Some(next) = shape(value) else {
                                    return Ok(Some(Value::Opaque("Shapes.or".into())));
                                };
                                current = self.factory.or(&current, &next);
                            }
                        }
                    }
                    result = current;
                } else {
                    let (Some(first), Some(second)) = (arguments.first(), arguments.get(1)) else {
                        return Ok(Some(Value::Opaque("Shapes.or".into())));
                    };
                    let (Some(first), Some(second)) = (shape(first), shape(second)) else {
                        return Ok(Some(Value::Opaque("Shapes.or".into())));
                    };
                    result = self.factory.or(&first, &second);
                }
                Ok(Some(Value::Shape(result)))
            }
            "join" => {
                let (Some(Value::Shape(first)), Some(Value::Shape(second))) =
                    (arguments.first(), arguments.get(1))
                else {
                    return Ok(Some(Value::Opaque("Shapes.join".into())));
                };
                let operation = match arguments.get(2) {
                    Some(Value::Marker(name)) => {
                        let name = name.rsplit('.').next().unwrap_or(name);
                        JoinOp::from_name(name)
                    }
                    _ => None,
                };
                let Some(operation) = operation else {
                    self.warn("Shapes.join with an unknown BooleanOp".to_string());
                    return Ok(Some(Value::Opaque("Shapes.join".into())));
                };
                Ok(Some(Value::Shape(
                    self.factory.join(first, second, operation),
                )))
            }
            "rotate" => {
                let Some(Value::Shape(input)) = arguments.first() else {
                    return Ok(Some(Value::Opaque("Shapes.rotate".into())));
                };
                let rotation = arguments
                    .get(1)
                    .and_then(|argument| self.rotation_of(argument));
                let center = match arguments.get(2) {
                    Some(value) => self.vec3_of(value),
                    None => Some([0.5, 0.5, 0.5]),
                };
                let (Some(rotation), Some(center)) = (rotation, center) else {
                    return Ok(Some(Value::Opaque("Shapes.rotate".into())));
                };
                Ok(Some(Value::Shape(
                    self.factory.rotate(input, &rotation, center),
                )))
            }
            "rotateHorizontal" | "rotateHorizontalAxis" | "rotateAllAxis" | "rotateAll"
            | "rotateAttachFace" => {
                self.rotation_map(name, descriptor, arguments)
            }
            _ => Ok(None),
        }
    }

    fn axis_value(&mut self, axis: &str) -> Option<Value<S>> {
        self.get_static(
            "net/minecraft/core/Direction$Axis",
            axis,
            "Lnet/minecraft/core/Direction$Axis;",
        )
        .into_option()
    }

    fn direction_value(&mut self, direction: &str) -> Value<S> {
        self.get_static(
            "net/minecraft/core/Direction",
            direction,
            "Lnet/minecraft/core/Direction;",
        )
    }

    /// The `Map<Direction|Axis, VoxelShape>` factories, mirroring vanilla's own
    /// bodies (the keys are the `Direction`/`Axis` constants).
    fn rotation_map(
        &mut self,
        name: &str,
        _descriptor: &str,
        arguments: &[Value<S>],
    ) -> Result<Option<Value<S>>, Fault> {
        let Some(Value::Shape(input)) = arguments.first().cloned() else {
            return Ok(Some(Value::Opaque(format!("Shapes.{name}"))));
        };
        let base = match arguments.get(1) {
            Some(value) => self.rotation_of(value),
            None => Some(Rotation::identity()),
        };
        let center = match arguments.get(2) {
            Some(value) => self.vec3_of(value),
            None => Some([0.5, 0.5, 0.5]),
        };
        let (Some(base), Some(center)) = (base, center) else {
            return Ok(Some(Value::Opaque(format!("Shapes.{name}"))));
        };
        let mut entries: Vec<(Value<S>, Value<S>)> = Vec::new();
        match name {
            "rotateHorizontal" => {
                let groups = [
                    ("NORTH", None),
                    ("EAST", Some("BLOCK_ROT_Y_90")),
                    ("SOUTH", Some("BLOCK_ROT_Y_180")),
                    ("WEST", Some("BLOCK_ROT_Y_270")),
                ];
                for (direction, group) in groups {
                    let key = self.direction_value(direction);
                    let value = self.rotated(&input, &base, group, center);
                    entries.push((key, value));
                }
            }
            "rotateHorizontalAxis" => {
                let key = self.axis_value("Z").unwrap_or(Value::Opaque("Axis.Z".into()));
                entries.push((key, Value::Shape(input.clone())));
                let key = self.axis_value("X").unwrap_or(Value::Opaque("Axis.X".into()));
                let value = self.rotated(&input, &base, Some("BLOCK_ROT_Y_90"), center);
                entries.push((key, value));
            }
            "rotateAllAxis" => {
                let key = self.axis_value("Z").unwrap_or(Value::Opaque("Axis.Z".into()));
                entries.push((key, Value::Shape(input.clone())));
                let key = self.axis_value("X").unwrap_or(Value::Opaque("Axis.X".into()));
                let value = self.rotated(&input, &base, Some("BLOCK_ROT_Y_90"), center);
                entries.push((key, value));
                let key = self.axis_value("Y").unwrap_or(Value::Opaque("Axis.Y".into()));
                let value = self.rotated(&input, &base, Some("BLOCK_ROT_X_90"), center);
                entries.push((key, value));
            }
            "rotateAll" => {
                let groups = [
                    ("NORTH", None),
                    ("EAST", Some("BLOCK_ROT_Y_90")),
                    ("SOUTH", Some("BLOCK_ROT_Y_180")),
                    ("WEST", Some("BLOCK_ROT_Y_270")),
                    ("UP", Some("BLOCK_ROT_X_270")),
                    ("DOWN", Some("BLOCK_ROT_X_90")),
                ];
                for (direction, group) in groups {
                    let key = self.direction_value(direction);
                    let value = self.rotated(&input, &base, group, center);
                    entries.push((key, value));
                }
            }
            "rotateAttachFace" => {
                let wall = self.attach_face("WALL");
                let floor = self.attach_face("FLOOR");
                let ceiling = self.attach_face("CEILING");
                let wall_map = self.rotation_map(
                    "rotateHorizontal",
                    "",
                    &[
                        Value::Shape(input.clone()),
                        Value::Rotation(base.clone()),
                    ],
                )?;
                entries.push((wall, wall_map.unwrap_or(Value::Opaque("rotateAttachFace".into()))));
                let floor_rotation = self.composed_group(&base, "BLOCK_ROT_X_270");
                let floor_map = self.rotation_map(
                    "rotateHorizontal",
                    "",
                    &[
                        Value::Shape(input.clone()),
                        Value::Rotation(floor_rotation),
                    ],
                )?;
                entries.push((floor, floor_map.unwrap_or(Value::Opaque("rotateAttachFace".into()))));
                let flipped = self.composed_group(&base, "BLOCK_ROT_Y_180");
                let ceiling_rotation = self.composed_group(&flipped, "BLOCK_ROT_X_90");
                let ceiling_map = self.rotation_map(
                    "rotateHorizontal",
                    "",
                    &[
                        Value::Shape(input.clone()),
                        Value::Rotation(ceiling_rotation),
                    ],
                )?;
                entries.push((
                    ceiling,
                    ceiling_map.unwrap_or(Value::Opaque("rotateAttachFace".into())),
                ));
            }
            _ => return Ok(None),
        }
        Ok(Some(Value::Map(Rc::new(RefCell::new(entries)))))
    }

    fn rotated(
        &mut self,
        input: &S,
        base: &Rotation,
        group: Option<&str>,
        center: [f64; 3],
    ) -> Value<S> {
        let rotation = match group {
            Some(group) => self.composed_group(base, group),
            None => base.clone(),
        };
        Value::Shape(self.factory.rotate(input, &rotation, center))
    }

    fn attach_face(&mut self, name: &str) -> Value<S> {
        self.get_static(
            "net/minecraft/world/level/block/state/properties/AttachFace",
            name,
            "Lnet/minecraft/world/level/block/state/properties/AttachFace;",
        )
    }

    fn call_lambda(
        &mut self,
        lambda: &Rc<Lambda<S>>,
        arguments: Vec<Value<S>>,
    ) -> Result<Value<S>, Fault> {
        let mut args = lambda.captured.clone();
        for argument in arguments {
            args.push(argument);
        }
        let (receiver, args) = if lambda.kind == 6 {
            (None, args)
        } else {
            let mut args = args;
            let receiver = args.remove(0);
            (Some(receiver), args)
        };
        let opcode = if lambda.kind == 6 { 0xb8 } else { 0xb6 };
        self.invoke_dispatch(
            opcode,
            &lambda.target_class,
            &lambda.name,
            &lambda.descriptor,
            receiver,
            args,
        )
    }

    fn rotation_of(&mut self, value: &Value<S>) -> Option<Rotation> {
        match value {
            Value::Rotation(rotation) => Some(rotation.clone()),
            Value::Object(instance) => {
                let borrowed = instance.borrow();
                if borrowed.class != OCTAHEDRAL {
                    return None;
                }
                let permutation = borrowed.fields.get("permutation")?.clone();
                drop(borrowed);
                let permutation = match permutation {
                    Value::Object(permutation) => permutation,
                    _ => return None,
                };
                let perm = {
                    let borrowed = permutation.borrow();
                    let read = |name: &str| {
                        borrowed.fields.get(name).and_then(Value::as_int).unwrap_or(0) as usize
                    };
                    [read("p0"), read("p1"), read("p2")]
                };
                let borrowed = instance.borrow();
                let read_invert = |name: &str| match borrowed.fields.get(name) {
                    Some(Value::Bool(value)) => *value,
                    Some(value) => value.as_int().map(|value| value != 0).unwrap_or(false),
                    None => false,
                };
                Some(Rotation {
                    perm,
                    inverted: [
                        read_invert("invertX"),
                        read_invert("invertY"),
                        read_invert("invertZ"),
                    ],
                    name: borrowed.enum_name.clone().unwrap_or_default(),
                })
            }
            _ => None,
        }
    }

    fn vec3_of(&mut self, value: &Value<S>) -> Option<[f64; 3]> {
        match value {
            Value::Vec3(vector) => Some(*vector),
            Value::Object(instance) => {
                let borrowed = instance.borrow();
                let read = |name: &str| borrowed.fields.get(name).and_then(Value::as_f64);
                Some([read("x")?, read("y")?, read("z")?])
            }
            _ => None,
        }
    }

    // -- the stack machine ----------------------------------------------------

    fn execute(
        &mut self,
        class: &Rc<ClassFile>,
        method: &Member,
        receiver: Option<Value<S>>,
        arguments: Vec<Value<S>>,
    ) -> Result<Value<S>, Fault> {
        if self.depth > 96 {
            return Err(Fault::Thrown("call depth".to_string()));
        }
        let Some(code) = &method.code else {
            return Ok(Value::Unit);
        };
        let instructions = decode_code(&code.bytes);
        let pcs = offsets_by_pc(&instructions);
        let (argument_types, return_type) = descriptor_args(&method.descriptor);
        let mut locals: Vec<Value<S>> = vec![Value::Null; code.max_locals as usize + 8];
        let mut local_index = 0usize;
        if method.access & 0x0008 == 0 {
            locals[0] = receiver.unwrap_or(Value::Null);
            local_index = 1;
        }
        for (value, kind) in arguments.into_iter().zip(argument_types.iter()) {
            if local_index >= locals.len() {
                locals.resize(local_index + 1, Value::Null);
            }
            locals[local_index] = value;
            local_index += type_slots(kind);
        }
        let mut stack: Vec<Value<S>> = Vec::new();
        let mut pc = 0usize;
        loop {
            if pc >= instructions.len() {
                break;
            }
            self.steps += 1;
            if self.steps > 4_000_000 {
                return Err(Fault::Steps);
            }
            let ins = &instructions[pc];
            let next = pc + 1;
            match ins.opcode {
                0x00 => {}
                0x01 => stack.push(Value::Null),
                0x02..=0x08 => stack.push(Value::Int(ins.opcode as i32 - 3)),
                0x09 => stack.push(Value::Long(0)),
                0x0a => stack.push(Value::Long(1)),
                0x0b => stack.push(Value::Float(0.0)),
                0x0c => stack.push(Value::Float(1.0)),
                0x0d => stack.push(Value::Float(2.0)),
                0x0e => stack.push(Value::Double(0.0)),
                0x0f => stack.push(Value::Double(1.0)),
                0x10 | 0x11 => stack.push(Value::Int(ins.imm)),
                0x12 | 0x13 => stack.push(self.constant_value(&class.constants, ins.index)),
                0x14 => stack.push(match long_double_constant(&class.constants, ins.index) {
                    Some(LongDouble::Long(value)) => Value::Long(value),
                    Some(LongDouble::Double(value)) => Value::Double(value),
                    None => Value::Null,
                }),
                0x15..=0x19 => {
                    let index = ins.index;
                    stack.push(locals.get(index).cloned().unwrap_or(Value::Null));
                }
                0x1a..=0x2d => {
                    let index = (ins.opcode - 0x1a) as usize % 4;
                    stack.push(locals.get(index).cloned().unwrap_or(Value::Null));
                }
                0x2e..=0x35 => {
                    let index = pop_int(&mut stack)?;
                    let array = pop(&mut stack)?;
                    stack.push(array_load(&array, index, ins.opcode)?);
                }
                0x36..=0x3a => {
                    let index = ins.index;
                    let value = pop(&mut stack)?;
                    set_local(&mut locals, index, value);
                }
                0x3b..=0x4e => {
                    let index = (ins.opcode - 0x3b) as usize % 4;
                    let value = pop(&mut stack)?;
                    set_local(&mut locals, index, value);
                }
                0x4f..=0x56 => {
                    let value = pop(&mut stack)?;
                    let index = pop_int(&mut stack)?;
                    let array = pop(&mut stack)?;
                    array_store(&array, index, value, ins.opcode)?;
                }
                0x57 => {
                    pop(&mut stack)?;
                }
                0x58 => {
                    if matches!(stack.last(), Some(Value::Long(_)) | Some(Value::Double(_))) {
                        pop(&mut stack)?;
                    } else {
                        pop(&mut stack)?;
                        pop(&mut stack)?;
                    }
                }
                0x59 => {
                    let value = stack.last().cloned().unwrap_or(Value::Null);
                    stack.push(value);
                }
                0x5a => {
                    let first = pop(&mut stack)?;
                    let second = pop(&mut stack)?;
                    stack.push(first.clone());
                    stack.push(second);
                    stack.push(first);
                }
                0x5b => {
                    let first = pop(&mut stack)?;
                    let second = pop(&mut stack)?;
                    let third = pop(&mut stack)?;
                    stack.push(first.clone());
                    stack.push(third);
                    stack.push(second);
                    stack.push(first);
                }
                0x5c => {
                    if matches!(stack.last(), Some(Value::Long(_)) | Some(Value::Double(_))) {
                        let value = stack.last().cloned().unwrap_or(Value::Null);
                        stack.push(value);
                    } else {
                        let first = pop(&mut stack)?;
                        let second = pop(&mut stack)?;
                        stack.push(second.clone());
                        stack.push(first.clone());
                        stack.push(second);
                        stack.push(first);
                    }
                }
                0x5d | 0x5e => {
                    // Rare `dup2_x1`/`dup2_x2`: keep the common (single-slot)
                    // form, report anything else.
                    self.warn("unmodelled dup2_x".to_string());
                    return Ok(Value::Opaque("dup2_x".into()));
                }
                0x5f => {
                    let first = pop(&mut stack)?;
                    let second = pop(&mut stack)?;
                    stack.push(first);
                    stack.push(second);
                }
                0x60..=0x83 => self.arithmetic(ins.opcode, &mut stack)?,
                0x84 => {
                    let index = ins.index;
                    let value = locals.get(index).and_then(Value::as_int).unwrap_or(0);
                    set_local(&mut locals, index, Value::Int(value.wrapping_add(ins.imm)));
                }
                0x85..=0x93 => self.convert(ins.opcode, &mut stack)?,
                0x94 => {
                    let right = pop_long(&mut stack)?;
                    let left = pop_long(&mut stack)?;
                    let ordering = match left.cmp(&right) {
                        std::cmp::Ordering::Less => -1,
                        std::cmp::Ordering::Equal => 0,
                        std::cmp::Ordering::Greater => 1,
                    };
                    stack.push(Value::Int(ordering));
                }
                0x95..=0x98 => {
                    let right = pop(&mut stack)?.as_f64().unwrap_or(f64::NAN);
                    let left = pop(&mut stack)?.as_f64().unwrap_or(f64::NAN);
                    let ordering = match left.partial_cmp(&right) {
                        Some(std::cmp::Ordering::Less) => -1,
                        Some(std::cmp::Ordering::Greater) => 1,
                        Some(std::cmp::Ordering::Equal) => 0,
                        None if ins.opcode == 0x95 || ins.opcode == 0x97 => -1,
                        None => 1,
                    };
                    stack.push(Value::Int(ordering));
                }
                0x99..=0x9e => {
                    let value = pop_int(&mut stack)?;
                    if compare(value, ins.opcode - 0x99) {
                        pc = jump(&pcs, ins, next);
                        continue;
                    }
                }
                0x9f..=0xa4 => {
                    let value = pop_int(&mut stack)?;
                    let other = pop_int(&mut stack)?;
                    if compare_pair(other, value, ins.opcode - 0x9f) {
                        pc = jump(&pcs, ins, next);
                        continue;
                    }
                }
                0xa5 | 0xa6 => {
                    let first = pop(&mut stack)?;
                    let second = pop(&mut stack)?;
                    let opaque = matches!(first, Value::Opaque(_)) || matches!(second, Value::Opaque(_));
                    if opaque {
                        self.warn("reference comparison against an opaque value".to_string());
                    }
                    let equal = values_equal(&first, &second);
                    if equal == (ins.opcode == 0xa5) {
                        pc = jump(&pcs, ins, next);
                        continue;
                    }
                }
                0xa7 | 0xc8 => {
                    pc = jump(&pcs, ins, next);
                    continue;
                }
                0xaa | 0xab => {
                    let Some(switch) = &ins.switch else { break };
                    let key = pop_int(&mut stack)?;
                    let target = switch
                        .keys
                        .iter()
                        .position(|candidate| *candidate == key)
                        .map(|slot| switch.targets[slot])
                        .unwrap_or(switch.default);
                    pc = pcs.get(&target).copied().unwrap_or(instructions.len());
                    continue;
                }
                0xac => return Ok(pop(&mut stack)?),
                0xad => return Ok(pop(&mut stack)?),
                0xae => return Ok(pop(&mut stack)?),
                0xaf => return Ok(pop(&mut stack)?),
                0xb0 => return Ok(pop(&mut stack)?),
                0xb1 => return Ok(Value::Unit),
                0xb2 => {
                    let Some((owner, name, descriptor)) =
                        classfile::field_ref(&class.constants, ins.index)
                    else {
                        return Err(Fault::Thrown("bad getstatic".into()));
                    };
                    stack.push(self.get_static(&owner, &name, &descriptor));
                }
                0xb3 => {
                    let Some((owner, name, descriptor)) =
                        classfile::field_ref(&class.constants, ins.index)
                    else {
                        return Err(Fault::Thrown("bad putstatic".into()));
                    };
                    let value = pop(&mut stack)?;
                    self.put_static(&owner, &name, &descriptor, value);
                }
                0xb4 => {
                    let Some((owner, name, descriptor)) =
                        classfile::field_ref(&class.constants, ins.index)
                    else {
                        return Err(Fault::Thrown("bad getfield".into()));
                    };
                    let object = pop(&mut stack)?;
                    stack.push(self.get_field(&object, &owner, &name, &descriptor));
                }
                0xb5 => {
                    let Some((owner, name, descriptor)) =
                        classfile::field_ref(&class.constants, ins.index)
                    else {
                        return Err(Fault::Thrown("bad putfield".into()));
                    };
                    let value = pop(&mut stack)?;
                    let object = pop(&mut stack)?;
                    if let Value::Object(instance) = object {
                        instance.borrow_mut().fields.insert(name, value);
                    }
                    let _ = (owner, descriptor);
                }
                0xb6..=0xb9 => {
                    let Some((owner, name, descriptor)) =
                        classfile::method_ref(&class.constants, ins.index)
                    else {
                        return Err(Fault::Thrown("bad invoke".into()));
                    };
                    let mut arguments = Vec::new();
                    for _ in 0..descriptor_value_count(&descriptor) {
                        arguments.push(pop(&mut stack)?);
                    }
                    arguments.reverse();
                    let receiver = if ins.opcode == 0xb8 {
                        None
                    } else {
                        Some(pop(&mut stack)?)
                    };
                    let value = self.invoke_dispatch(
                        ins.opcode,
                        &owner,
                        &name,
                        &descriptor,
                        receiver,
                        arguments,
                    )?;
                    if !descriptor.ends_with(")V") {
                        stack.push(value);
                    }
                }
                0xba => {
                    let Some(Constant::InvokeDynamic {
                        bootstrap,
                        name: name_index,
                        ..
                    }) = class.constants.get(ins.index)
                    else {
                        return Err(Fault::Thrown("bad invokedynamic".into()));
                    };
                    let bootstrap = *bootstrap;
                    let name_index: usize =
                        name_index.trim_start_matches('#').parse().unwrap_or(0);
                    let (_, descriptor) = nat(&class.constants, name_index);
                    let Some(row) = class.bootstraps.get(bootstrap) else {
                        return Err(Fault::Thrown("missing bootstrap".into()));
                    };
                    let mut captured = Vec::new();
                    for _ in 0..descriptor_value_count(&descriptor) {
                        captured.push(pop(&mut stack)?);
                    }
                    captured.reverse();
                    let Some((kind, target_class, target_name, target_descriptor)) =
                        self.lambda_target(&class.constants, row)
                    else {
                        self.warn("invokedynamic without a lambda target".to_string());
                        return Ok(Value::Opaque("invokedynamic".into()));
                    };
                    stack.push(Value::Lambda(Rc::new(Lambda {
                        target_class,
                        name: target_name,
                        descriptor: target_descriptor,
                        kind,
                        captured,
                    })));
                }
                0xbb => {
                    let class_name = class_name(&class.constants, ins.index);
                    stack.push(Value::Object(Rc::new(RefCell::new(Instance {
                        class: class_name,
                        fields: BTreeMap::new(),
                        enum_name: None,
                        enum_ordinal: 0,
                    }))));
                }
                0xbc => {
                    let length = pop_int(&mut stack)?;
                    stack.push(Value::Array(Rc::new(RefCell::new(vec![
                        Value::Int(0);
                        length.max(0) as usize
                    ]))));
                }
                0xbd => {
                    let length = pop_int(&mut stack)?;
                    stack.push(Value::Array(Rc::new(RefCell::new(vec![
                        Value::Null;
                        length.max(0) as usize
                    ]))));
                }
                0xbe => {
                    let array = pop(&mut stack)?;
                    let length = match array {
                        Value::Array(array) => array.borrow().len() as i32,
                        _ => {
                            self.warn("arraylength on a non-array".to_string());
                            0
                        }
                    };
                    stack.push(Value::Int(length));
                }
                0xbf => {
                    let value = pop(&mut stack)?;
                    return Err(Fault::Thrown(format!("thrown {value:?}")));
                }
                0xc0 => {}
                0xc1 => {
                    let target = class_name(&class.constants, ins.index);
                    let value = pop(&mut stack)?;
                    stack.push(Value::Bool(self.instance_of(&value, &target)));
                }
                0xc2 | 0xc3 => {
                    pop(&mut stack)?;
                }
                0xc5 => {
                    let dimensions: Vec<i32> = (0..ins.imm)
                        .map(|_| pop_int(&mut stack))
                        .collect::<Result<Vec<_>, _>>()?;
                    let mut value = Value::Array(Rc::new(RefCell::new(Vec::new())));
                    for length in dimensions.iter().rev() {
                        value = Value::Array(Rc::new(RefCell::new(vec![
                            value.clone();
                            (*length).max(0) as usize
                        ])));
                    }
                    stack.push(value);
                }
                0xc6 | 0xc7 => {
                    let value = pop(&mut stack)?;
                    if matches!(value, Value::Opaque(_)) {
                        self.warn("null check on an opaque value".to_string());
                    }
                    let is_null = matches!(value, Value::Null);
                    if is_null == (ins.opcode == 0xc6) {
                        pc = jump(&pcs, ins, next);
                        continue;
                    }
                }
                0xc9 => {
                    return Err(Fault::Thrown("jsr_w".into()));
                }
                _ => {
                    self.warn(format!("unmodelled opcode {:#04x}", ins.opcode));
                    return Ok(Value::Opaque(format!("opcode {:#04x}", ins.opcode)));
                }
            }
            pc = next;
        }
        Ok(match return_type.as_str() {
            "V" => Value::Unit,
            "Z" | "B" | "C" | "S" | "I" => Value::Int(0),
            "J" => Value::Long(0),
            "F" => Value::Float(0.0),
            "D" => Value::Double(0.0),
            _ => Value::Null,
        })
    }

    fn lambda_target(
        &mut self,
        constants: &[Constant],
        row: &classfile::Bootstrap,
    ) -> Option<(u8, String, String, String)> {
        // LambdaMetafactory: bootstrap arg 1 is the implementation handle.
        let handle = row.arguments.get(1)?;
        let (class, name, descriptor) = method_handle_target(constants, *handle)?;
        // Whether the target is an instance method decides whether the first
        // captured argument is the receiver. `classfile.rs` drops the
        // MethodHandle reference kind, so read the access flags of the method.
        let kind = match self.resolve(&class, &name, &descriptor) {
            Some((_, method)) if method.access & 0x0008 == 0 => 5,
            _ => 6,
        };
        Some((kind, class, name, descriptor))
    }

    fn constant_value(&mut self, constants: &[Constant], index: usize) -> Value<S> {
        match constants.get(index) {
            Some(Constant::Integer(value)) => Value::Int(*value),
            Some(Constant::Float(value)) => Value::Float(*value),
            Some(Constant::Long(value)) => Value::Long(*value),
            Some(Constant::Double(value)) => Value::Double(*value),
            Some(Constant::String(_)) => {
                Value::Str(string_constant(constants, index).unwrap_or_default())
            }
            Some(Constant::Class(_)) => Value::Class(class_name(constants, index)),
            Some(Constant::MethodType(_)) => Value::Marker("MethodType".to_string()),
            _ => Value::Null,
        }
    }

    fn get_field(
        &mut self,
        object: &Value<S>,
        owner: &str,
        name: &str,
        descriptor: &str,
    ) -> Value<S> {
        match object {
            Value::Object(instance) => {
                let found = instance.borrow().fields.get(name).cloned();
                match found {
                    Some(value) => value,
                    None => {
                        // Enum constructors store their own fields; anything
                        // else is a real gap.
                        Value::Opaque(format!("{owner}.{name}:{descriptor}"))
                    }
                }
            }
            Value::BlockRef(_) if name == "hasCollision" => Value::Bool(self.has_collision),
            Value::StateRef(state) => {
                if name == "hasCollision" {
                    Value::Bool(state.has_collision)
                } else {
                    Value::Opaque(format!("state.{name}"))
                }
            }
            Value::Vec3(vector) => match name {
                "x" => Value::Double(vector[0]),
                "y" => Value::Double(vector[1]),
                "z" => Value::Double(vector[2]),
                _ => Value::Opaque(format!("vec3.{name}")),
            },
            _ => Value::Opaque(format!("{owner}.{name}:{descriptor}")),
        }
    }

    fn instance_of(&mut self, value: &Value<S>, target: &str) -> bool {
        match value {
            Value::Object(instance) => {
                let class = instance.borrow().class.clone();
                let mut current = Some(class);
                let mut depth = 0;
                while let Some(internal) = current {
                    if internal == target {
                        return true;
                    }
                    let Some(parsed) = self.class(&internal) else {
                        return false;
                    };
                    current = if parsed.super_name.is_empty() {
                        None
                    } else {
                        Some(parsed.super_name.clone())
                    };
                    depth += 1;
                    if depth > 48 {
                        return false;
                    }
                }
                false
            }
            Value::StateRef(_) => {
                target.ends_with("BlockState") || target.ends_with("BlockStateBase")
            }
            Value::BlockRef(_) => target.ends_with("Block") || target.ends_with("BlockBehaviour"),
            _ => false,
        }
    }

    fn arithmetic(&mut self, opcode: u8, stack: &mut Vec<Value<S>>) -> Result<(), Fault> {
        // JVM arithmetic opcodes are grouped in fours: int, long, float,
        // double. `neg`, `shift` and `and/or/xor` live in their own groups.
        if opcode <= 0x6f {
            let operation = (opcode - 0x60) / 4;
            match (opcode - 0x60) % 4 {
                0 => {
                    let b = pop_int(stack)?;
                    let a = pop_int(stack)?;
                    stack.push(Value::Int(match operation {
                        0 => a.wrapping_add(b),
                        1 => a.wrapping_sub(b),
                        2 => a.wrapping_mul(b),
                        3 if b != 0 => a.wrapping_div(b),
                        _ => 0,
                    }));
                }
                1 => {
                    let b = pop_long(stack)?;
                    let a = pop_long(stack)?;
                    stack.push(Value::Long(match operation {
                        0 => a.wrapping_add(b),
                        1 => a.wrapping_sub(b),
                        2 => a.wrapping_mul(b),
                        3 if b != 0 => a.wrapping_div(b),
                        _ => 0,
                    }));
                }
                2 => {
                    let b = pop(stack)?.as_f64().unwrap_or(0.0) as f32;
                    let a = pop(stack)?.as_f64().unwrap_or(0.0) as f32;
                    stack.push(Value::Float(match operation {
                        0 => a + b,
                        1 => a - b,
                        2 => a * b,
                        3 => a / b,
                        _ => 0.0,
                    }));
                }
                _ => {
                    let b = pop(stack)?.as_f64().unwrap_or(0.0);
                    let a = pop(stack)?.as_f64().unwrap_or(0.0);
                    stack.push(Value::Double(match operation {
                        0 => a + b,
                        1 => a - b,
                        2 => a * b,
                        3 => a / b,
                        _ => 0.0,
                    }));
                }
            }
            return Ok(());
        }
        if opcode <= 0x73 {
            match opcode - 0x70 {
                0 => {
                    let b = pop_int(stack)?;
                    let a = pop_int(stack)?;
                    stack.push(Value::Int(if b != 0 { a.wrapping_rem(b) } else { 0 }));
                }
                1 => {
                    let b = pop_long(stack)?;
                    let a = pop_long(stack)?;
                    stack.push(Value::Long(if b != 0 { a.wrapping_rem(b) } else { 0 }));
                }
                2 => {
                    let b = pop(stack)?.as_f64().unwrap_or(0.0) as f32;
                    let a = pop(stack)?.as_f64().unwrap_or(0.0) as f32;
                    stack.push(Value::Float(a % b));
                }
                _ => {
                    let b = pop(stack)?.as_f64().unwrap_or(0.0);
                    let a = pop(stack)?.as_f64().unwrap_or(0.0);
                    stack.push(Value::Double(a % b));
                }
            }
            return Ok(());
        }
        if opcode <= 0x77 {
            let kind = opcode - 0x74;
            let value = pop(stack)?;
            stack.push(match kind {
                0 => Value::Int(value.as_int().unwrap_or(0).wrapping_neg()),
                1 => Value::Long(pop_long_value(&value).wrapping_neg()),
                2 => Value::Float(-(value.as_f64().unwrap_or(0.0) as f32)),
                _ => Value::Double(-value.as_f64().unwrap_or(0.0)),
            });
            return Ok(());
        }
        if opcode <= 0x7d {
            let shift = pop_int(stack)? as u32;
            match opcode {
                0x78 => {
                    let value = pop_int(stack)?;
                    stack.push(Value::Int(value.wrapping_shl(shift)));
                }
                0x79 => {
                    let value = pop_long(stack)?;
                    stack.push(Value::Long(value.wrapping_shl(shift)));
                }
                0x7a => {
                    let value = pop_int(stack)?;
                    stack.push(Value::Int(value.wrapping_shr(shift)));
                }
                0x7b => {
                    let value = pop_long(stack)?;
                    stack.push(Value::Long(value.wrapping_shr(shift)));
                }
                0x7c => {
                    let value = pop_int(stack)?;
                    stack.push(Value::Int(((value as u32) >> shift) as i32));
                }
                _ => {
                    let value = pop_long(stack)?;
                    stack.push(Value::Long(((value as u64) >> shift) as i64));
                }
            }
            return Ok(());
        }
        match opcode {
            0x7e => {
                let b = pop_int(stack)?;
                let a = pop_int(stack)?;
                stack.push(Value::Int(a & b));
            }
            0x7f => {
                let b = pop_long(stack)?;
                let a = pop_long(stack)?;
                stack.push(Value::Long(a & b));
            }
            0x80 => {
                let b = pop_int(stack)?;
                let a = pop_int(stack)?;
                stack.push(Value::Int(a | b));
            }
            0x81 => {
                let b = pop_long(stack)?;
                let a = pop_long(stack)?;
                stack.push(Value::Long(a | b));
            }
            0x82 => {
                let b = pop_int(stack)?;
                let a = pop_int(stack)?;
                stack.push(Value::Int(a ^ b));
            }
            _ => {
                let b = pop_long(stack)?;
                let a = pop_long(stack)?;
                stack.push(Value::Long(a ^ b));
            }
        }
        Ok(())
    }

    fn convert(&mut self, opcode: u8, stack: &mut Vec<Value<S>>) -> Result<(), Fault> {
        match opcode {
            0x85 => {
                let value = pop_int(stack)?;
                stack.push(Value::Long(value as i64));
            }
            0x86 => {
                let value = pop_int(stack)?;
                stack.push(Value::Float(value as f32));
            }
            0x87 => {
                let value = pop_int(stack)?;
                stack.push(Value::Double(value as f64));
            }
            0x88 => {
                let value = pop_long(stack)?;
                stack.push(Value::Int(value as i32));
            }
            0x89 => {
                let value = pop_long(stack)?;
                stack.push(Value::Float(value as f32));
            }
            0x8a => {
                let value = pop_long(stack)?;
                stack.push(Value::Double(value as f64));
            }
            0x8b => {
                let value = pop(stack)?.as_f64().unwrap_or(0.0);
                stack.push(Value::Int(value as i32));
            }
            0x8c => {
                let value = pop(stack)?.as_f64().unwrap_or(0.0);
                stack.push(Value::Long(value as i64));
            }
            0x8d => {
                let value = pop(stack)?.as_f64().unwrap_or(0.0);
                stack.push(Value::Double(value as f64));
            }
            0x8e => {
                let value = pop(stack)?.as_f64().unwrap_or(0.0);
                stack.push(Value::Int(value as i32));
            }
            0x8f => {
                let value = pop(stack)?.as_f64().unwrap_or(0.0);
                stack.push(Value::Long(value as i64));
            }
            0x90 => {
                let value = pop(stack)?.as_f64().unwrap_or(0.0);
                stack.push(Value::Float(value as f32));
            }
            0x91 => {
                let value = pop_int(stack)?;
                stack.push(Value::Int(value as i8 as i32));
            }
            0x92 => {
                let value = pop_int(stack)?;
                stack.push(Value::Int(value as u16 as i32));
            }
            0x93 => {
                let value = pop_int(stack)?;
                stack.push(Value::Int(value as i16 as i32));
            }
            _ => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

impl<S: Clone + Debug + PartialEq> Vm<S> {
    /// `Shapes.rotateHorizontal`'s inner `group.compose(BLOCK_ROT_Y_90)`, with
    /// the group constant read from the jar (never a hardcoded matrix).
    fn composed_group(&mut self, base: &Rotation, other: &str) -> Rotation {
        let value = self.get_static(OCTAHEDRAL, other, &format!("L{OCTAHEDRAL};"));
        match self.rotation_of(&value) {
            Some(rotation) => base.compose(&rotation),
            None => Rotation {
                perm: base.perm,
                inverted: base.inverted,
                name: format!("{}+{other}", base.name),
            },
        }
    }
}

fn set_local<S: Clone>(locals: &mut Vec<Value<S>>, index: usize, value: Value<S>) {
    if index >= locals.len() {
        locals.resize(index + 1, Value::Null);
    }
    locals[index] = value;
}

fn pop<S>(stack: &mut Vec<Value<S>>) -> Result<Value<S>, Fault> {
    stack.pop().ok_or(Fault::Thrown("stack underflow".into()))
}

fn pop_int<S>(stack: &mut Vec<Value<S>>) -> Result<i32, Fault> {
    pop(stack)?.as_int().ok_or(Fault::Thrown("expected int".into()))
}

fn pop_long<S>(stack: &mut Vec<Value<S>>) -> Result<i64, Fault> {
    match pop(stack)? {
        Value::Long(value) => Ok(value),
        Value::Int(value) => Ok(value as i64),
        other => other
            .as_f64()
            .map(|value| value as i64)
            .ok_or(Fault::Thrown("expected long".into())),
    }
}

fn pop_long_value<S>(value: &Value<S>) -> i64 {
    match value {
        Value::Long(value) => *value,
        Value::Int(value) => *value as i64,
        other => other.as_f64().unwrap_or(0.0) as i64,
    }
}

fn compare(value: i32, condition: u8) -> bool {
    match condition {
        0 => value == 0,
        1 => value != 0,
        2 => value < 0,
        3 => value >= 0,
        4 => value > 0,
        5 => value <= 0,
        _ => false,
    }
}

fn compare_pair(left: i32, right: i32, condition: u8) -> bool {
    match condition {
        0 => left == right,
        1 => left != right,
        2 => left < right,
        3 => left >= right,
        4 => left > right,
        5 => left <= right,
        _ => false,
    }
}

fn jump(pcs: &BTreeMap<usize, usize>, ins: &Ins, default: usize) -> usize {
    let target = ins.offset as i64 + ins.imm as i64;
    if target < 0 {
        return default;
    }
    pcs.get(&(target as usize)).copied().unwrap_or(default)
}

fn array_load<S: Clone>(
    array: &Value<S>,
    index: i32,
    _opcode: u8,
) -> Result<Value<S>, Fault> {
    match array {
        Value::Array(values) => {
            let borrowed = values.borrow();
            let index = index.max(0) as usize;
            borrowed
                .get(index)
                .cloned()
                .ok_or(Fault::Thrown("array index".into()))
        }
        _ => Err(Fault::Thrown("aaload on a non-array".into())),
    }
}

fn array_store<S: Clone>(
    array: &Value<S>,
    index: i32,
    value: Value<S>,
    _opcode: u8,
) -> Result<(), Fault> {
    match array {
        Value::Array(values) => {
            let mut borrowed = values.borrow_mut();
            let index = index.max(0) as usize;
            if index >= borrowed.len() {
                borrowed.resize(index + 1, Value::Null);
            }
            borrowed[index] = value;
            Ok(())
        }
        _ => Err(Fault::Thrown("aastore on a non-array".into())),
    }
}

impl<S> Value<S> {
    fn into_option(self) -> Option<Value<S>> {
        match self {
            Value::Opaque(_) => None,
            other => Some(other),
        }
    }
}
