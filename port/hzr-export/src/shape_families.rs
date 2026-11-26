//! Per-class block shape selectors for the families vanilla computes in block
//! code, rebuilt from the player's own `client.jar`.
//!
//! `cached-collision.nbt` / `occlusion.nbt` index one `VoxelShape` per state.
//! `geometry.rs` derives the default path (`getShape` -> `Shapes.block()`,
//! `noCollision` -> `Shapes.empty()`); everything that overrides a shape method
//! is a gap there. This module closes the gaps for the families whose shape is a
//! pure function of the state's properties: stairs, slabs, walls, fences, panes,
//! doors, trapdoors, candles, vines, multiface blocks, the plant families,
//! carpets, snow layers, ladders, chains and lanterns.
//!
//! # VoxelShape API contract
//!
//! This module builds on `src/voxelshape.rs` (owned by the geometry step; not
//! edited here). The contract it relies on, documented so the two files can be
//! reviewed independently:
//!
//! * `VoxelShape::empty()` / `VoxelShape::block()` are vanilla's `Shapes.empty()`
//!   and `Shapes.block()` singletons (they drive the `canonical` byte).
//! * `VoxelShape::block_box(x1,y1,z1,x2,y2,z2)` is `Block.box` in 1/16 units,
//!   `Shapes.create`'s `findBits` partition included: coordinates on 2^-k
//!   (k <= 3) become a `CubeVoxelShape` grid, finer coordinates an
//!   `ArrayVoxelShape` with one cell, a degenerate axis becomes `empty()`.
//! * `VoxelShape::from_boxes(&[[f64;6]])` is `Shapes.or` folded over the boxes
//!   (each element `Shapes.box`), i.e. `joinUnoptimized(...).optimize()`.
//!
//! All rotations below are vanilla `Shapes.rotate`/`rotateHorizontal`/
//! `rotateAll` expressed on boxes: a vanilla `OctahedralGroup` permutes the axes
//! (`out[i] = in[permutation.p[i]]`) and then optionally mirrors an axis around
//! the block centre (`out[i] = 1 - out[i]`). Rotating an axis-aligned box is
//! exact, so the union geometry matches vanilla's rotated `ArrayVoxelShape`.
//!
//! # Coverage
//!
//! `family_for_class` names the classes handled; `shape_for_class` returns
//! `None` for anything else, which is how callers keep the gap report honest.
//! The builder-side table derivations (which block uses which class) are not
//! repeated here: `block::registry` already has the class per block.
//!
//! Handled, per-class (with the method each table is taken from):
//!
//! | class(es) | collision | occlusion |
//! | --- | --- | --- |
//! | `StairBlock` | `getShape` | same (`getCollisionShape`/`getOcclusionShape` default to it) |
//! | `SlabBlock` | `getShape` | same |
//! | `WallBlock` | `getCollisionShape` (post 24 high) | `getShape` (post 16 high) |
//! | `FenceBlock` | `getCollisionShape` (24 high) | `getOcclusionShape` (thin low bars) |
//! | `IronBarsBlock` | `getCollisionShape` | `getShape` |
//! | `DoorBlock`, `TrapDoorBlock` | `getShape` | same |
//! | `CandleBlock` | `getShape` (1..4 candles) | same |
//! | `VineBlock`, `MultifaceBlock` (+`GlowLichenBlock`, `SculkVeinBlock`, ...) | `noCollision` -> empty | `getShape` (face plates, all-false -> `Shapes.block()`) |
//! | `*BushBlock`, `FlowerBlock`, `TallGrassBlock`, `SaplingBlock`, `DryVegetationBlock`, `Short/TallDryGrassBlock`, `Nether/HangingRootsBlock`, `BaseCoralPlantBlock`, `SugarCaneBlock`, `CropBlock`, `StemBlock`, `SweetBerryBushBlock`, `AzaleaBlock` | `noCollision` -> empty | `getShape` (plant columns; flowers carry their `OffsetType.XZ` offset) |
//! | `CarpetBlock`/`WoolCarpetBlock`, `SnowLayerBlock`, `LadderBlock`, `ChainBlock`, `LanternBlock` | `getShape`/`getCollisionShape` | same |
//!
//! Deliberately **not** here:
//!
//! * `LeavesBlock` (and the other `noOcclusion()` full blocks): neither class
//!   overrides a shape method, so `geometry.rs`'s default path already derives
//!   their collision and occlusion `Shapes.block()`. They need no selector.
//! * `FenceGateBlock` (its three shape methods are separate functions), the
//!   beds, doors' neighbours such as `TrapDoorBlock`-like custom blocks,
//!   mushrooms, dripleaves, `AttachedStemBlock`'s diagonal shape, and the rest of
//!   the 133 classes the test reports: they stay on the gap list for the
//!   bytecode-VM step.

#![allow(dead_code, clippy::duplicate_mod)]

#[path = "voxelshape.rs"]
mod voxelshape;

pub use voxelshape::{EncodedShape, VoxelShape};

/// Which of the two shape tables is being asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    /// `BlockState.getCollisionShape` (`hasCollision` false => empty).
    Collision,
    /// `BlockBehaviour.getOcclusionShape` (defaults to `getShape`).
    Occlusion,
}

/// One state's property values, as `(name, value)` pairs.
///
/// Value names are vanilla's `Property.getPossibleValues()` names
/// (`north`, `low`, `bottom`, `straight`, `1`..`4`, ...), which is what
/// `states.bin`/`block-order.json` carry.
pub struct State<'a> {
    pairs: &'a [(&'a str, &'a str)],
}

impl<'a> State<'a> {
    pub fn new(pairs: &'a [(&'a str, &'a str)]) -> State<'a> {
        State { pairs }
    }

    pub fn text(&self, name: &str) -> Option<&'a str> {
        self.pairs
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| *value)
    }

    pub fn boolean(&self, name: &str) -> Option<bool> {
        match self.text(name)? {
            // `BooleanProperty.create(name)` lists `true, false` in this version.
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    }

    pub fn int(&self, name: &str) -> Option<i32> {
        self.text(name)?.parse().ok()
    }
}

/// A vanilla `OctahedralGroup` restricted to the eight rotations this module
/// uses: `out[i] = in[p[i]]`, then mirrored around 0.5 where `inv[i]` is set.
#[derive(Clone, Copy, Debug)]
struct Rotation {
    p: [usize; 3],
    inv: [bool; 3],
}

impl Rotation {
    const IDENTITY: Rotation = Rotation {
        p: [0, 1, 2],
        inv: [false, false, false],
    };
    /// `BLOCK_ROT_Y_90` (`ROT_90_Y_NEG`): `(x,y,z) -> (1-z, y, x)`.
    const Y90: Rotation = Rotation {
        p: [2, 1, 0],
        inv: [true, false, false],
    };
    /// `BLOCK_ROT_Y_180` (`ROT_180_FACE_XZ`): `(x,y,z) -> (1-x, y, 1-z)`.
    const Y180: Rotation = Rotation {
        p: [0, 1, 2],
        inv: [true, false, true],
    };
    /// `BLOCK_ROT_Y_270` (`ROT_90_Y_POS`): `(x,y,z) -> (z, y, 1-x)`.
    const Y270: Rotation = Rotation {
        p: [2, 1, 0],
        inv: [false, false, true],
    };
    /// `BLOCK_ROT_X_270` (`ROT_90_X_POS`): `(x,y,z) -> (x, 1-z, y)`.
    const X270: Rotation = Rotation {
        p: [0, 2, 1],
        inv: [false, true, false],
    };
    /// `BLOCK_ROT_X_90` (`ROT_90_X_NEG`): `(x,y,z) -> (x, z, 1-y)`.
    const X90: Rotation = Rotation {
        p: [0, 2, 1],
        inv: [false, false, true],
    };
    /// `INVERT_Y`: `(x,y,z) -> (x, 1-y, z)`.
    const INVERT_Y: Rotation = Rotation {
        p: [0, 1, 2],
        inv: [false, true, false],
    };
}

/// A box in block units, `[x1, y1, z1, x2, y2, z2]`.
type Box6 = [f64; 6];

fn rotate_point(rotation: Rotation, point: [f64; 3]) -> [f64; 3] {
    let mut out = [
        point[rotation.p[0]],
        point[rotation.p[1]],
        point[rotation.p[2]],
    ];
    for axis in 0..3 {
        if rotation.inv[axis] {
            out[axis] = 1.0 - out[axis];
        }
    }
    out
}

fn rotate_box(rotation: Rotation, b: Box6) -> Box6 {
    let corners = [
        [b[0], b[1], b[2]],
        [b[0], b[1], b[5]],
        [b[0], b[4], b[2]],
        [b[0], b[4], b[5]],
        [b[3], b[1], b[2]],
        [b[3], b[1], b[5]],
        [b[3], b[4], b[2]],
        [b[3], b[4], b[5]],
    ];
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for corner in corners {
        let point = rotate_point(rotation, corner);
        for axis in 0..3 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }
    [min[0], min[1], min[2], max[0], max[1], max[2]]
}

fn rotate_boxes(rotation: Rotation, boxes: &[Box6]) -> Vec<Box6> {
    boxes.iter().map(|b| rotate_box(rotation, *b)).collect()
}

/// `VoxelShape.move(Vec3)` on a box list.
fn translate(boxes: &mut [Box6], dx: f64, dy: f64, dz: f64) {
    for b in boxes.iter_mut() {
        b[0] += dx;
        b[1] += dy;
        b[2] += dz;
        b[3] += dx;
        b[4] += dy;
        b[5] += dz;
    }
}

fn union(boxes: &[Box6]) -> VoxelShape {
    VoxelShape::from_boxes(boxes)
}

// 1/16-unit box helpers, mirroring `Block.box`/`Block.column`/`Block.boxZ`.

fn box16(x1: f64, y1: f64, z1: f64, x2: f64, y2: f64, z2: f64) -> Box6 {
    [
        x1 / 16.0,
        y1 / 16.0,
        z1 / 16.0,
        x2 / 16.0,
        y2 / 16.0,
        z2 / 16.0,
    ]
}

/// `Block.column(radius, yMin, yMax)`: centred square column.
fn column_16(radius: f64, y1: f64, y2: f64) -> Box6 {
    let half = radius / 2.0;
    box16(8.0 - half, y1, 8.0 - half, 8.0 + half, y2, 8.0 + half)
}

/// `Block.boxZ(depth, y1, y2, z1, z2)`: centred-depth slab along z.
fn box_z(depth: f64, y1: f64, y2: f64, z1: f64, z2: f64) -> Box6 {
    let half = depth / 2.0;
    box16(8.0 - half, y1, z1, 8.0 + half, y2, z2)
}

/// `Block.boxZ(size, z1, z2)`: centred square bar along z.
fn box_z_square(size: f64, z1: f64, z2: f64) -> Box6 {
    let half = size / 2.0;
    box16(8.0 - half, 8.0 - half, z1, 8.0 + half, 8.0 + half, z2)
}

/// `Block.cube(sizeX, sizeY, sizeZ)`: centred box `sizeX x sizeY x sizeZ`.
fn cube_16(size_x: f64, size_y: f64, size_z: f64) -> Box6 {
    let half = size_y / 2.0;
    box16(
        8.0 - size_x / 2.0,
        8.0 - half,
        8.0 - size_z / 2.0,
        8.0 + size_x / 2.0,
        8.0 + half,
        8.0 + size_z / 2.0,
    )
}

fn horizontal_rotation(name: &str) -> Option<Rotation> {
    match name {
        "north" => Some(Rotation::IDENTITY),
        "east" => Some(Rotation::Y90),
        "south" => Some(Rotation::Y180),
        "west" => Some(Rotation::Y270),
        _ => None,
    }
}

fn direction_rotation(name: &str) -> Option<Rotation> {
    match name {
        "up" => Some(Rotation::X270),
        "down" => Some(Rotation::X90),
        _ => horizontal_rotation(name),
    }
}

/// `Direction.getCounterClockWise` for the four horizontal directions.
fn counter_clockwise(name: &str) -> &'static str {
    match name {
        "north" => "west",
        "west" => "south",
        "south" => "east",
        _ => "north",
    }
}

/// `Direction.getClockWise` for the four horizontal directions.
fn clockwise(name: &str) -> &'static str {
    match name {
        "north" => "east",
        "east" => "south",
        "south" => "west",
        _ => "north",
    }
}

// ---------------------------------------------------------------------------
// Families
// ---------------------------------------------------------------------------

/// The class families this module handles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Family {
    Stairs,
    Slab,
    Wall,
    Fence,
    Pane,
    Door,
    TrapDoor,
    Candle,
    Vine,
    Multiface,
    Carpet,
    SnowLayer,
    Ladder,
    Chain,
    Lantern,
    /// `BushBlock`-style `Block.column(16, 0, height)` plant.
    Bush,
    /// `Block.column(radius, 0, height)` plant.
    Column {
        radius: f64,
        height: f64,
    },
    /// `FlowerBlock`: `Block.column(6, 0, 10)` moved by the state's
    /// `OffsetType.XZ` offset; at `BlockPos.ZERO` (`Mth.getSeed(0,0,0) == 0`)
    /// that is `(-0.25, 0, -0.25)`.
    Flower,
    /// `Block.column(radius, y_min, y_max)` plant (hanging roots).
    ColumnRange {
        radius: f64,
        y_min: f64,
        y_max: f64,
    },
    /// A `Block.column(radius, 0, height)` shape whose blocks *do* collide
    /// (lily pad, farmland/path).
    FullColumn {
        radius: f64,
        height: f64,
    },
    /// `AzaleaBlock`: `or(column(16, 8, 16), column(4, 0, 8))`.
    Azalea,
    /// `CropBlock`: `column(16, 0, 2 + 2 * age)`.
    Crop,
    /// `CarrotBlock`/`PotatoBlock`: `column(16, 0, 2 + age)`.
    CropLinear,
    /// `StemBlock`: `column(2, 0, 2 + 2 * age)`.
    Stem,
    /// `SweetBerryBushBlock`: sapling / growing / full-block by age.
    SweetBerryBush,
}

/// Every class name this module understands, with the family it maps to.
pub const CLASSES: &[(&str, Family)] = &[
    ("StairBlock", Family::Stairs),
    ("WeatheringCopperStairBlock", Family::Stairs),
    ("SlabBlock", Family::Slab),
    ("WeatheringCopperSlabBlock", Family::Slab),
    ("WallBlock", Family::Wall),
    ("FenceBlock", Family::Fence),
    ("IronBarsBlock", Family::Pane),
    ("WeatheringCopperBarsBlock", Family::Pane),
    ("DoorBlock", Family::Door),
    ("WeatheringCopperDoorBlock", Family::Door),
    ("TrapDoorBlock", Family::TrapDoor),
    ("WeatheringCopperTrapDoorBlock", Family::TrapDoor),
    ("CandleBlock", Family::Candle),
    ("VineBlock", Family::Vine),
    ("MultifaceBlock", Family::Multiface),
    ("GlowLichenBlock", Family::Multiface),
    ("SculkVeinBlock", Family::Multiface),
    ("MultifaceSpreadeableBlock", Family::Multiface),
    ("CarpetBlock", Family::Carpet),
    ("WoolCarpetBlock", Family::Carpet),
    ("SnowLayerBlock", Family::SnowLayer),
    ("LadderBlock", Family::Ladder),
    ("ChainBlock", Family::Chain),
    ("WeatheringCopperChainBlock", Family::Chain),
    ("LanternBlock", Family::Lantern),
    ("WeatheringLanternBlock", Family::Lantern),
    ("BushBlock", Family::Bush),
    ("FlowerBlock", Family::Flower),
    (
        "TallGrassBlock",
        Family::Column {
            radius: 12.0,
            height: 13.0,
        },
    ),
    (
        "SaplingBlock",
        Family::Column {
            radius: 12.0,
            height: 12.0,
        },
    ),
    (
        "DryVegetationBlock",
        Family::Column {
            radius: 12.0,
            height: 13.0,
        },
    ),
    (
        "ShortDryGrassBlock",
        Family::Column {
            radius: 12.0,
            height: 10.0,
        },
    ),
    (
        "TallDryGrassBlock",
        Family::Column {
            radius: 14.0,
            height: 16.0,
        },
    ),
    (
        "NetherRootsBlock",
        Family::Column {
            radius: 12.0,
            height: 13.0,
        },
    ),
    (
        "BaseCoralPlantBlock",
        Family::Column {
            radius: 12.0,
            height: 15.0,
        },
    ),
    (
        "SugarCaneBlock",
        Family::Column {
            radius: 12.0,
            height: 16.0,
        },
    ),
    (
        "HangingRootsBlock",
        Family::ColumnRange {
            radius: 12.0,
            y_min: 10.0,
            y_max: 16.0,
        },
    ),
    (
        "CoralPlantBlock",
        Family::Column {
            radius: 12.0,
            height: 15.0,
        },
    ),
    (
        "NetherSproutsBlock",
        Family::Column {
            radius: 12.0,
            height: 3.0,
        },
    ),
    (
        "CactusFlowerBlock",
        Family::Column {
            radius: 14.0,
            height: 12.0,
        },
    ),
    (
        "SeagrassBlock",
        Family::Column {
            radius: 12.0,
            height: 12.0,
        },
    ),
    (
        "TallSeagrassBlock",
        Family::Column {
            radius: 12.0,
            height: 16.0,
        },
    ),
    (
        "KelpBlock",
        Family::Column {
            radius: 16.0,
            height: 9.0,
        },
    ),
    (
        "LilyPadBlock",
        Family::FullColumn {
            radius: 14.0,
            height: 1.5,
        },
    ),
    (
        "FarmlandBlock",
        Family::FullColumn {
            radius: 16.0,
            height: 15.0,
        },
    ),
    (
        "PathBlock",
        Family::FullColumn {
            radius: 16.0,
            height: 15.0,
        },
    ),
    ("StainedGlassPaneBlock", Family::Pane),
    ("CarrotBlock", Family::CropLinear),
    ("PotatoBlock", Family::CropLinear),
    ("BeetrootBlock", Family::Crop),
    ("WitherRoseBlock", Family::Flower),
    ("EyeblossomBlock", Family::Flower),
    ("AzaleaBlock", Family::Azalea),
    ("CropBlock", Family::Crop),
    ("StemBlock", Family::Stem),
    ("SweetBerryBushBlock", Family::SweetBerryBush),
];

/// The family of a block class, or `None` when this module does not cover it.
pub fn family_for_class(class: &str) -> Option<Family> {
    CLASSES
        .iter()
        .find(|(name, _)| *name == class)
        .map(|(_, family)| *family)
}

/// Whether a family's blocks are registered with `noCollision()` (so their
/// collision shape is empty even though their outline is not).
pub fn collision_is_empty(family: Family) -> bool {
    matches!(
        family,
        Family::Vine
            | Family::Multiface
            | Family::Flower
            | Family::Bush
            | Family::Column { .. }
            | Family::ColumnRange { .. }
            | Family::Crop
            | Family::CropLinear
            | Family::Stem
            | Family::SweetBerryBush
    )
}

/// The shape of one state, or `None` when the class is not covered.
pub fn shape_for_class(class: &str, state: &State, kind: ShapeKind) -> Option<VoxelShape> {
    let family = family_for_class(class)?;
    Some(shape_for_family(family, state, kind))
}

/// The shape of one state of a family.
pub fn shape_for_family(family: Family, state: &State, kind: ShapeKind) -> VoxelShape {
    if kind == ShapeKind::Collision && collision_is_empty(family) {
        return VoxelShape::empty();
    }
    match family {
        Family::Stairs => stairs(state),
        Family::Slab => slab(state),
        Family::Wall => wall(state, kind),
        Family::Fence => fence(state, kind),
        Family::Pane => pane(state, kind),
        Family::Door => door(state),
        Family::TrapDoor => trapdoor(state),
        Family::Candle => candle(state),
        Family::Vine => empty_boxes_are_a_block(vine(state)),
        Family::Multiface => empty_boxes_are_a_block(multiface(state)),
        Family::Carpet => union(&[column_16(16.0, 0.0, 1.0)]),
        Family::SnowLayer => snow_layer(state, kind),
        Family::Ladder => ladder(state),
        Family::Chain => chain(state),
        Family::Lantern => lantern(state),
        Family::Bush => union(&[column_16(16.0, 0.0, 13.0)]),
        Family::Column { radius, height } => union(&[column_16(radius, 0.0, height)]),
        Family::Flower => {
            let mut boxes = [column_16(6.0, 0.0, 10.0)];
            // `FlowerBlock.getShape` returns `SHAPE.move(state.getOffset(pos))`;
            // the shape tables were queried at `BlockPos.ZERO`.
            translate(&mut boxes, -0.25, 0.0, -0.25);
            union(&boxes)
        }
        Family::ColumnRange {
            radius,
            y_min,
            y_max,
        } => union(&[column_16(radius, y_min, y_max)]),
        Family::FullColumn { radius, height } => union(&[column_16(radius, 0.0, height)]),
        Family::Azalea => azalea(),
        Family::Crop => crop(state, 2.0),
        Family::CropLinear => crop(state, 1.0),
        Family::Stem => stem(state),
        Family::SweetBerryBush => sweet_berry_bush(state),
    }
}

// ---------------------------------------------------------------------------
// Stairs (`StairBlock`)
// ---------------------------------------------------------------------------

/// `SHAPE_OUTER` = half slab + top quarter; `SHAPE_STRAIGHT` adds its Y90
/// rotation (top half), `SHAPE_INNER` one more (three top quarters). The maps
/// are `Shapes.rotateHorizontal(shape)` and its `INVERT_Y` variant.
fn stair_base(label: &str) -> Vec<Box6> {
    let slab = box16(0.0, 0.0, 0.0, 16.0, 8.0, 16.0);
    let outer = box16(0.0, 8.0, 0.0, 8.0, 16.0, 8.0);
    let straight = box16(8.0, 8.0, 0.0, 16.0, 16.0, 8.0);
    let inner = box16(8.0, 8.0, 8.0, 16.0, 16.0, 16.0);
    match label {
        "outer_left" | "outer_right" => vec![slab, outer],
        "inner_left" | "inner_right" => vec![slab, outer, straight, inner],
        _ => vec![slab, outer, straight],
    }
}

fn stairs(state: &State) -> VoxelShape {
    let half = state.text("half").unwrap_or("bottom");
    let facing = state.text("facing").unwrap_or("north");
    let shape = state.text("shape").unwrap_or("straight");
    // The map lookup key, straight from StairBlock.getShape's switch.
    let key = match shape {
        "inner_left" => counter_clockwise(facing),
        "outer_right" => clockwise(facing),
        _ => facing,
    };
    let Some(rotation) = horizontal_rotation(key) else {
        return VoxelShape::empty();
    };
    let boxes = stair_base(shape);
    let boxes = rotate_boxes(rotation, &boxes);
    if half == "top" {
        union(&rotate_boxes(Rotation::INVERT_Y, &boxes))
    } else {
        union(&boxes)
    }
}

// ---------------------------------------------------------------------------
// Slabs (`SlabBlock`)
// ---------------------------------------------------------------------------

fn slab(state: &State) -> VoxelShape {
    match state.text("type") {
        Some("top") => union(&[column_16(16.0, 8.0, 16.0)]),
        Some("double") => VoxelShape::block(),
        _ => union(&[column_16(16.0, 0.0, 8.0)]),
    }
}

// ---------------------------------------------------------------------------
// Walls (`WallBlock`)
// ---------------------------------------------------------------------------

/// `WallBlock.makeShapes(postHeight, sideHeight)`: the post is
/// `column(8, 0, postHeight)`, a low side `boxZ(6, 0, sideHeight, 0, 11)` and a
/// tall side `boxZ(6, 0, postHeight, 0, 11)`, both rotated horizontally. The
/// lambda starts from the post when `up` and ORs one piece per horizontal
/// property that is not `none`.
fn wall_boxes(state: &State, post_height: f64, side_height: f64) -> Vec<Box6> {
    let mut boxes: Vec<Box6> = Vec::new();
    if state.boolean("up").unwrap_or(false) {
        boxes.push(column_16(8.0, 0.0, post_height));
    }
    for direction in ["north", "east", "south", "west"] {
        let Some(value) = state.text(direction) else {
            continue;
        };
        let piece = match value {
            "low" => box_z(6.0, 0.0, side_height, 0.0, 11.0),
            "tall" => box_z(6.0, 0.0, post_height, 0.0, 11.0),
            _ => continue,
        };
        let Some(rotation) = horizontal_rotation(direction) else {
            continue;
        };
        boxes.push(rotate_box(rotation, piece));
    }
    boxes
}

fn wall(state: &State, kind: ShapeKind) -> VoxelShape {
    let boxes = match kind {
        // `collisionShapes` = makeShapes(24, 24).
        ShapeKind::Collision => wall_boxes(state, 24.0, 24.0),
        // `getShape` = makeShapes(16, 14); also the occlusion shape (WallBlock
        // does not override `getOcclusionShape`).
        ShapeKind::Occlusion => wall_boxes(state, 16.0, 14.0),
    };
    union(&boxes)
}

// ---------------------------------------------------------------------------
// Fences (`FenceBlock` / `CrossCollisionBlock`)
// ---------------------------------------------------------------------------

/// `CrossCollisionBlock.makeShapes(nodeWidth, nodeHeight, extensionWidth,
/// extensionHeight, extensionY)`: post `column(nodeWidth, 0, nodeHeight)`,
/// extension `boxZ(extensionWidth, extensionHeight, extensionY, 0, 8)` rotated
/// horizontally, ORed for every true `north`/`east`/`south`/`west`.
fn cross_boxes(
    state: &State,
    post_width: f64,
    post_height: f64,
    ext_width: f64,
    ext_y1: f64,
    ext_y2: f64,
) -> Vec<Box6> {
    let mut boxes = vec![column_16(post_width, 0.0, post_height)];
    for direction in ["north", "east", "south", "west"] {
        if !state.boolean(direction).unwrap_or(false) {
            continue;
        }
        let Some(rotation) = horizontal_rotation(direction) else {
            continue;
        };
        boxes.push(rotate_box(
            rotation,
            box_z(ext_width, ext_y1, ext_y2, 0.0, 8.0),
        ));
    }
    boxes
}

fn fence(state: &State, kind: ShapeKind) -> VoxelShape {
    match kind {
        // FenceBlock passes (4, 16, 4, 16, 24) to CrossCollisionBlock:
        // collisionShapes = makeShapes(4, 24, 4, 0, 24).
        ShapeKind::Collision => union(&cross_boxes(state, 4.0, 24.0, 4.0, 0.0, 24.0)),
        // getOcclusionShape = makeShapes(4, 16, 2, 6, 15). (`getShape`, the
        // outline, is makeShapes(4, 16, 4, 0, 16) but neither table stores it.)
        ShapeKind::Occlusion => union(&cross_boxes(state, 4.0, 16.0, 2.0, 6.0, 15.0)),
    }
}

// ---------------------------------------------------------------------------
// Iron bars / panes (`IronBarsBlock`)
// ---------------------------------------------------------------------------

fn pane(state: &State, _kind: ShapeKind) -> VoxelShape {
    // IronBarsBlock passes (2, 16, 2, 16, 16): collision = makeShapes(2, 16, 2,
    // 0, 16), outline = makeShapes(2, 16, 2, 0, 16), occlusion = getShape.
    union(&cross_boxes(state, 2.0, 16.0, 2.0, 0.0, 16.0))
}

// ---------------------------------------------------------------------------
// Doors and trapdoors
// ---------------------------------------------------------------------------

/// `DoorBlock.SHAPES = Shapes.rotateHorizontal(Block.boxZ(16, 13, 16))`, i.e. a
/// 3/16 slab at the block edge keyed by direction; `getShape` maps facing (or a
/// rotation of it when open) through it.
fn door(state: &State) -> VoxelShape {
    let facing = state.text("facing").unwrap_or("north");
    let open = state.boolean("open").unwrap_or(false);
    let hinge = state.text("hinge").unwrap_or("left");
    let key = if open {
        if hinge == "right" {
            counter_clockwise(facing)
        } else {
            clockwise(facing)
        }
    } else {
        facing
    };
    let Some(rotation) = horizontal_rotation(key) else {
        return VoxelShape::empty();
    };
    union(&rotate_boxes(rotation, &[box_z_square(16.0, 13.0, 16.0)]))
}

/// `TrapDoorBlock.SHAPES = Shapes.rotateAll(Block.boxZ(16, 13, 16))`; closed
/// gates pick the horizontal slab by half, open ones the facing rotation.
fn trapdoor(state: &State) -> VoxelShape {
    let open = state.boolean("open").unwrap_or(false);
    let key = if open {
        state.text("facing").unwrap_or("north")
    } else if state.text("half") == Some("top") {
        "down"
    } else {
        "up"
    };
    let Some(rotation) = direction_rotation(key) else {
        return VoxelShape::empty();
    };
    union(&rotate_boxes(rotation, &[box_z_square(16.0, 13.0, 16.0)]))
}

// ---------------------------------------------------------------------------
// Candles (`CandleBlock`)
// ---------------------------------------------------------------------------

/// `CandleBlock.SHAPES[candles - 1]`, straight from the static initialiser.
fn candle(state: &State) -> VoxelShape {
    match state.int("candles").unwrap_or(1) {
        1 => union(&[column_16(2.0, 0.0, 6.0)]),
        2 => union(&[box16(5.0, 0.0, 6.0, 11.0, 6.0, 9.0)]),
        3 => union(&[box16(5.0, 0.0, 6.0, 10.0, 6.0, 11.0)]),
        _ => union(&[box16(5.0, 0.0, 5.0, 11.0, 6.0, 10.0)]),
    }
}

// ---------------------------------------------------------------------------
// Vines and multiface blocks
// ---------------------------------------------------------------------------

/// A plate on the block face named by a direction, with the vanilla
/// `Block.boxZ(16, 0, 1)` base (a 1/16 plate on the north face).
fn face_plate(direction: &str) -> Option<Box6> {
    let rotation = direction_rotation(direction)?;
    Some(rotate_box(rotation, box_z_square(16.0, 0.0, 1.0)))
}

/// `VineBlock`/`MultifaceBlock` return `Shapes.block()` when no face is set:
/// `shape.isEmpty() ? Shapes.block() : shape`.
fn empty_boxes_are_a_block(shape: VoxelShape) -> VoxelShape {
    if shape.is_empty() {
        VoxelShape::block()
    } else {
        shape
    }
}

fn vine(state: &State) -> VoxelShape {
    let mut boxes = Vec::new();
    for direction in ["north", "east", "south", "west", "up"] {
        if !state.boolean(direction).unwrap_or(false) {
            continue;
        }
        if let Some(plate) = face_plate(direction) {
            boxes.push(plate);
        }
    }
    union(&boxes)
}

fn multiface(state: &State) -> VoxelShape {
    let mut boxes = Vec::new();
    for direction in ["down", "up", "north", "south", "west", "east"] {
        if !state.boolean(direction).unwrap_or(false) {
            continue;
        }
        if let Some(plate) = face_plate(direction) {
            boxes.push(plate);
        }
    }
    union(&boxes)
}

// ---------------------------------------------------------------------------
// Other thin/plant-like families
// ---------------------------------------------------------------------------

/// `SnowLayerBlock.SHAPES = Block.boxes(8, i -> Block.column(16, 0, 2 * i))`;
/// `getShape` uses the layer count, `getCollisionShape` the count minus one.
fn snow_layer(state: &State, kind: ShapeKind) -> VoxelShape {
    let layers = state.int("layers").unwrap_or(1);
    // SHAPES has indices 0..=8 (Block.boxes uses rangeClosed); getShape indexes
    // at `layers`, collision at `layers - 1`.
    let index = if kind == ShapeKind::Collision {
        (layers - 1).max(0)
    } else {
        layers
    };
    union(&[column_16(16.0, 0.0, 2.0 * index as f64)])
}

/// `LadderBlock.SHAPES = Shapes.rotateHorizontal(Block.boxZ(16, 13, 16))`.
fn ladder(state: &State) -> VoxelShape {
    let facing = state.text("facing").unwrap_or("north");
    let Some(rotation) = horizontal_rotation(facing) else {
        return VoxelShape::empty();
    };
    union(&rotate_boxes(rotation, &[box_z_square(16.0, 13.0, 16.0)]))
}

/// `ChainBlock.SHAPES = Shapes.rotateAllAxis(Block.cube(3, 3, 16))`:
/// `Map.of(Z, shape, X, rotate(shape, Y90), Y, rotate(shape, X90))`.
fn chain(state: &State) -> VoxelShape {
    let base = cube_16(3.0, 3.0, 16.0);
    let rotation = match state.text("axis").unwrap_or("y") {
        "x" => Rotation::Y90,
        "z" => Rotation::IDENTITY,
        _ => Rotation::X90,
    };
    union(&[rotate_box(rotation, base)])
}

/// `LanternBlock`: standing = `or(column(4, 7, 9), column(6, 0, 7))`, hanging is
/// the standing shape moved up 1/16 then optimised.
fn lantern(state: &State) -> VoxelShape {
    if state.boolean("hanging").unwrap_or(false) {
        union(&[column_16(4.0, 8.0, 10.0), column_16(6.0, 1.0, 8.0)])
    } else {
        union(&[column_16(4.0, 7.0, 9.0), column_16(6.0, 0.0, 7.0)])
    }
}

fn azalea() -> VoxelShape {
    union(&[column_16(16.0, 8.0, 16.0), column_16(4.0, 0.0, 8.0)])
}

/// `CropBlock.SHAPES[age] = Block.column(16, 0, 2 + factor * age)`;
/// `BeetrootBlock` uses the 2x factor, `CarrotBlock`/`PotatoBlock` the 1x one.
fn crop(state: &State, factor: f64) -> VoxelShape {
    let age = state.int("age").unwrap_or(0);
    union(&[column_16(16.0, 0.0, 2.0 + factor * age as f64)])
}

/// `StemBlock.SHAPES[age] = Block.column(2, 0, 2 + 2 * age)`.
fn stem(state: &State) -> VoxelShape {
    let age = state.int("age").unwrap_or(0);
    union(&[column_16(2.0, 0.0, 2.0 + 2.0 * age as f64)])
}

/// `SweetBerryBushBlock`: age 0 -> `column(10, 0, 8)`, age 3 ->
/// `Shapes.block()`, otherwise `column(14, 0, 16)`.
fn sweet_berry_bush(state: &State) -> VoxelShape {
    match state.int("age").unwrap_or(0) {
        0 => union(&[column_16(10.0, 0.0, 8.0)]),
        3 => VoxelShape::block(),
        _ => union(&[column_16(14.0, 0.0, 16.0)]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slab_maps_its_three_types() {
        let top = [("type", "top"), ("waterlogged", "false")];
        let bottom = [("type", "bottom"), ("waterlogged", "true")];
        let double = [("type", "double"), ("waterlogged", "false")];
        assert_eq!(
            shape_for_family(Family::Slab, &State::new(&top), ShapeKind::Collision)
                .encode()
                .canonical,
            0
        );
        assert_eq!(
            shape_for_family(Family::Slab, &State::new(&double), ShapeKind::Collision)
                .encode()
                .canonical,
            2
        );
        let bottom_shape =
            shape_for_family(Family::Slab, &State::new(&bottom), ShapeKind::Collision);
        assert_eq!(bottom_shape.encode().y, vec![0.0, 0.5, 1.0]);
        assert!(
            !shape_for_family(Family::Slab, &State::new(&top), ShapeKind::Collision).is_empty()
        );
    }

    #[test]
    fn a_candle_counts_up_to_four_shapes() {
        let one = [("candles", "1"), ("lit", "false"), ("waterlogged", "false")];
        let four = [("candles", "4"), ("lit", "false"), ("waterlogged", "false")];
        let one = shape_for_family(Family::Candle, &State::new(&one), ShapeKind::Collision);
        let four = shape_for_family(Family::Candle, &State::new(&four), ShapeKind::Collision);
        assert_eq!(one.encode().x, vec![0.4375, 0.5625]);
        assert_eq!(four.encode().x, vec![0.3125, 0.6875]);
    }

    #[test]
    fn plants_without_collision_are_empty_only_for_the_collision_table() {
        let pairs = [("age", "0")];
        let state = State::new(&pairs);
        assert!(shape_for_family(Family::Crop, &state, ShapeKind::Collision).is_empty());
        assert!(!shape_for_family(Family::Crop, &state, ShapeKind::Occlusion).is_empty());
    }

    #[test]
    fn horizontal_rotations_form_a_cycle() {
        assert_eq!(counter_clockwise("north"), "west");
        assert_eq!(counter_clockwise("west"), "south");
        assert_eq!(clockwise("north"), "east");
        assert_eq!(clockwise("east"), "south");
    }
}
