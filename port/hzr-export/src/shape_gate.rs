//! Fence-gate and cave-vine shape selectors: the two shape families vanilla
//! computes in block code that `shape_families` deliberately leaves out.
//!
//! `cached-collision.nbt` / `occlusion.nbt` index one `VoxelShape` per state.
//! `geometry.rs` derives the default path and `shape_families` models the big
//! geometric families; this module closes the next gap with the decoded
//! `client.jar` bytecode of the two classes:
//!
//! # `FenceGateBlock`
//!
//! The static initialiser builds six shapes (`javap -p -c` of
//! `net/minecraft/world/level/block/FenceGateBlock`):
//!
//! ```text
//! SHAPES               = Shapes.rotateHorizontalAxis(Block.cube(16, 16, 4))
//! SHAPES_WALL          = mapValues(SHAPES, s -> Shapes.join(s, Block.column(16, 13, 16), BooleanOp.ONLY_FIRST))
//! SHAPE_COLLISION      = Shapes.rotateHorizontalAxis(Block.column(16, 4, 0, 24))
//! SHAPE_SUPPORT        = Shapes.rotateHorizontalAxis(Block.column(16, 4, 5, 24))
//! SHAPE_OCCLUSION      = Shapes.rotateHorizontalAxis(Shapes.or(Block.box(0, 5, 7, 2, 16, 9),
//!                                                              Block.box(14, 5, 7, 16, 16, 9)))
//! SHAPE_OCCLUSION_WALL = mapValues(SHAPE_OCCLUSION, s -> s.move(0, -0.1875, 0).optimize())
//! ```
//!
//! The `Block` helpers are the 1/16-unit factories: `cube(16,16,4)` =
//! `box(0,0,6,16,16,10)`, `column(radius, y1, y2)` = `box(8-r/2, y1, 8-r/2,
//! 8+r/2, y2, 8+r/2)` and `column(xSize, zSize, y1, y2)` = `box(0, y1,
//! 8-zSize/2, xSize, y2, 8+zSize/2)`, so `column(16, 4, 0, 24)` is
//! `box(0,0,6,16,24,10)`. `Block.column(16, 13, 16)` is the three-argument
//! overload and means y 13..16 (`box(0,13,0,16,16,16)`), not a 16-high column:
//! `BooleanOp.ONLY_FIRST` is `left && !right`, i.e. the wall gate is the gate
//! clipped *below* 13/16.
//!
//! | method | shape map | key |
//! | --- | --- | --- |
//! | `getShape` | `IN_WALL ? SHAPES_WALL : SHAPES` | `facing.axis` |
//! | `getCollisionShape` | `OPEN ? Shapes.empty() : SHAPE_COLLISION` | `facing.axis` |
//! | `getBlockSupportShape` | `OPEN ? Shapes.empty() : SHAPE_SUPPORT` | `facing.axis` |
//! | `getOcclusionShape` | `IN_WALL ? SHAPE_OCCLUSION_WALL : SHAPE_OCCLUSION` | `facing.axis` |
//!
//! `POWERED` never changes a shape. `rotateHorizontalAxis` keys its map by
//! `Direction.Axis`, so north/south (Z) keep the listed shape and east/west (X)
//! get `BLOCK_ROT_Y_90`, `(x,y,z) -> (1-z, y, x)`.
//!
//! # Cave vines
//!
//! `CaveVinesBlock` (head) and `CaveVinesPlantBlock` (body) inherit
//! `GrowingPlantBlock.getShape`, which returns the constructor's `shape` field;
//! both pass `CaveVines.SHAPE = Block.column(14, 0, 16)` =
//! `box(1,0,1,15,16,15)`. Neither class overrides a shape method beyond that,
//! and the blocks are registered `noCollision()`, so the collision shape is
//! `Shapes.empty()` and the default support shape (which delegates to the
//! collision shape) is empty too; the occlusion shape is `getShape`. `age` and
//! `berries` do not change the shape.
//!
//! # Validation
//!
//! `tests/shape_gate.rs` compares every covered state against the prepared
//! copies in `tmp/ship-extract`: all 470 states (13 fence gates x 32 states,
//! 52 cave-vine heads, 2 bodies) are exact in both `cached-collision.nbt` and
//! `occlusion.nbt` (940/940 state-shapes), and the 470 support shapes match
//! `worldgen/blocks/cached-support.nbt` when that table is present. The outline
//! tables are not part of the bundle, so `Method::Outline` is pinned by the
//! decoded formulas in the unit tests below.
//!
//! Deliberately not here: the other `GrowingPlantBlock` families (`KelpBlock`,
//! `KelpPlantBlock`, `WeepingVinesBlock`/`PlantBlock`,
//! `TwistingVinesBlock`/`PlantBlock`), which carry different shape constants
//! and are separate selectors. The test reports them as gaps; nothing is
//! guessed.

#![allow(dead_code, clippy::duplicate_mod)]

#[path = "shape_families.rs"]
mod shape_families;

pub use shape_families::{EncodedShape, State, VoxelShape};

/// The shape method of one state that a caller wants.
///
/// `Collision` and `Occlusion` are the two methods the prepared tables store;
/// `Outline` (`getShape`) and `Support` (`getBlockSupportShape`) are decoded
/// here because the same selectors need them for the other two tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// `BlockState.getShape`: the block's outline.
    Outline,
    /// `BlockBehaviour.getCollisionShape` (`hasCollision == false` => empty).
    Collision,
    /// `BlockBehaviour.getBlockSupportShape`.
    Support,
    /// `BlockBehaviour.getOcclusionShape`.
    Occlusion,
}

impl Method {
    /// The method name, for coverage reports.
    pub fn name(self) -> &'static str {
        match self {
            Method::Outline => "getShape",
            Method::Collision => "getCollisionShape",
            Method::Support => "getBlockSupportShape",
            Method::Occlusion => "getOcclusionShape",
        }
    }
}

/// The two shape families this module covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// `FenceGateBlock`: a gate keyed by `facing.axis`, height 16 or 13 in a wall.
    FenceGate,
    /// `CaveVinesBlock` / `CaveVinesPlantBlock`: a 14/16 column, no collision.
    CaveVines,
}

/// Every class this module understands, with its family.
pub const CLASSES: &[(&str, Family)] = &[
    ("FenceGateBlock", Family::FenceGate),
    ("CaveVinesBlock", Family::CaveVines),
    ("CaveVinesPlantBlock", Family::CaveVines),
];

/// The family of a block class, or `None` when this module does not cover it.
pub fn family_for_class(class: &str) -> Option<Family> {
    CLASSES
        .iter()
        .find(|(name, _)| *name == class)
        .map(|(_, family)| *family)
}

/// The shape of one state of a covered class, or `None` for a class outside this
/// module (callers keep the gap report honest).
pub fn shape_for_class(class: &str, state: &State, method: Method) -> Option<VoxelShape> {
    family_for_class(class).map(|family| shape_for_family(family, state, method))
}

/// The shape of one state of a family.
pub fn shape_for_family(family: Family, state: &State, method: Method) -> VoxelShape {
    match family {
        Family::FenceGate => fence_gate(state, method),
        Family::CaveVines => cave_vines(method),
    }
}

// ---------------------------------------------------------------------------
// Box helpers (same 1/16-unit conventions as `shape_families`)
// ---------------------------------------------------------------------------

/// A box in block units, `[x1, y1, z1, x2, y2, z2]`.
type Box6 = [f64; 6];

const EPSILON: f64 = 1e-7;

/// `Block.box` in 1/16 units.
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

/// `Block.column(radius, y1, y2)`: a centred square column.
fn column16(radius: f64, y1: f64, y2: f64) -> Box6 {
    let half = radius / 2.0;
    box16(8.0 - half, y1, 8.0 - half, 8.0 + half, y2, 8.0 + half)
}

/// `Block.column(xSize, zSize, yMin, yMax)`: a column with the given footprint
/// (the four-argument overload the fence gate's collision/support shapes use).
fn column_xz16(x_size: f64, z_size: f64, y1: f64, y2: f64) -> Box6 {
    box16(
        8.0 - x_size / 2.0,
        y1,
        8.0 - z_size / 2.0,
        8.0 + x_size / 2.0,
        y2,
        8.0 + z_size / 2.0,
    )
}

/// `Block.cube(sizeX, sizeY, sizeZ)`: a centred `sizeX x sizeY x sizeZ` box.
fn cube16(size_x: f64, size_y: f64, size_z: f64) -> Box6 {
    box16(
        8.0 - size_x / 2.0,
        8.0 - size_y / 2.0,
        8.0 - size_z / 2.0,
        8.0 + size_x / 2.0,
        8.0 + size_y / 2.0,
        8.0 + size_z / 2.0,
    )
}

/// `Shapes.or` over a box list (each element `Shapes.box`).
fn union(boxes: &[Box6]) -> VoxelShape {
    VoxelShape::from_boxes(boxes)
}

/// `VoxelShape.move(Vec3)` on a box list, the `optimize()` half of
/// `s.move(...).optimize()` being the `union`/`Shapes.or` rebuild at the call
/// site.
fn move_boxes(boxes: &[Box6], dx: f64, dy: f64, dz: f64) -> Vec<Box6> {
    boxes
        .iter()
        .map(|b| {
            [
                b[0] + dx,
                b[1] + dy,
                b[2] + dz,
                b[3] + dx,
                b[4] + dy,
                b[5] + dz,
            ]
        })
        .collect()
}

/// `Shapes.join(a, b, BooleanOp.ONLY_FIRST)` (`a && !b`) on two box lists: cut
/// every piece of `a` on `b`'s bounds and keep what lies outside `b`, then
/// rebuild through `Shapes.or`. The fence-gate operands are single boxes, where
/// this is the exact `optimize()` result.
fn join_only_first(a: &[Box6], b: &[Box6]) -> VoxelShape {
    let mut pieces = a.to_vec();
    for cutter in b {
        let mut next = Vec::with_capacity(pieces.len() * 6);
        for piece in &pieces {
            next.extend(subtract_box(*piece, *cutter));
        }
        pieces = next;
    }
    union(&pieces)
}

/// `piece \ cutter` for two boxes: the six disjoint slabs around their overlap.
fn subtract_box(piece: Box6, cutter: Box6) -> Vec<Box6> {
    let overlap = [
        piece[0].max(cutter[0]),
        piece[1].max(cutter[1]),
        piece[2].max(cutter[2]),
        piece[3].min(cutter[3]),
        piece[4].min(cutter[4]),
        piece[5].min(cutter[5]),
    ];
    if overlap[3] - overlap[0] < EPSILON
        || overlap[4] - overlap[1] < EPSILON
        || overlap[5] - overlap[2] < EPSILON
    {
        return vec![piece];
    }
    let [x1, y1, z1, x2, y2, z2] = overlap;
    let slabs = [
        // Beside the cutter on X.
        [piece[0], piece[1], piece[2], x1, piece[4], piece[5]],
        [x2, piece[1], piece[2], piece[3], piece[4], piece[5]],
        // Beside it on Y.
        [x1, piece[1], piece[2], x2, y1, piece[5]],
        [x1, y2, piece[2], x2, piece[4], piece[5]],
        // Beside it on Z.
        [x1, y1, piece[2], x2, y2, z1],
        [x1, y1, z2, x2, y2, piece[5]],
    ];
    slabs
        .into_iter()
        .filter(|slab| {
            slab[3] - slab[0] >= EPSILON
                && slab[4] - slab[1] >= EPSILON
                && slab[5] - slab[2] >= EPSILON
        })
        .collect()
}

// ---------------------------------------------------------------------------
// `Shapes.rotateHorizontalAxis`
// ---------------------------------------------------------------------------

/// A vanilla `OctahedralGroup` restricted to the quarter turns this module uses:
/// `out[i] = in[p[i]]`, then mirrored around 0.5 where `inv[i]` is set.
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
        let mut point = [
            corner[rotation.p[0]],
            corner[rotation.p[1]],
            corner[rotation.p[2]],
        ];
        for axis in 0..3 {
            if rotation.inv[axis] {
                point[axis] = 1.0 - point[axis];
            }
        }
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

/// The `Direction.Axis` key of `rotateHorizontalAxis`: north/south are Z (the
/// listed shape), east/west are X (`BLOCK_ROT_Y_90`). A missing/unknown facing
/// falls back to the Z entry, which is also vanilla's default state.
fn facing_rotation(facing: Option<&str>) -> Rotation {
    match facing {
        Some("east") | Some("west") => Rotation::Y90,
        _ => Rotation::IDENTITY,
    }
}

// ---------------------------------------------------------------------------
// `FenceGateBlock`
// ---------------------------------------------------------------------------

fn fence_gate(state: &State, method: Method) -> VoxelShape {
    let rotation = facing_rotation(state.text("facing"));
    let open = state.boolean("open").unwrap_or(false);
    let in_wall = state.boolean("in_wall").unwrap_or(false);
    match method {
        // `(IN_WALL ? SHAPES_WALL : SHAPES).get(facing.axis)`; open/powered do
        // not affect the outline.
        Method::Outline => {
            let gate = rotate_boxes(rotation, &[cube16(16.0, 16.0, 4.0)]);
            if in_wall {
                // `Shapes.join(gate, Block.column(16, 13, 16), ONLY_FIRST)`.
                join_only_first(&gate, &[column16(16.0, 13.0, 16.0)])
            } else {
                union(&gate)
            }
        }
        // `OPEN ? Shapes.empty() : SHAPE_COLLISION.get(facing.axis)`.
        Method::Collision => {
            if open {
                VoxelShape::empty()
            } else {
                union(&rotate_boxes(
                    rotation,
                    &[column_xz16(16.0, 4.0, 0.0, 24.0)],
                ))
            }
        }
        // `OPEN ? Shapes.empty() : SHAPE_SUPPORT.get(facing.axis)`.
        Method::Support => {
            if open {
                VoxelShape::empty()
            } else {
                union(&rotate_boxes(
                    rotation,
                    &[column_xz16(16.0, 4.0, 5.0, 24.0)],
                ))
            }
        }
        // `(IN_WALL ? SHAPE_OCCLUSION_WALL : SHAPE_OCCLUSION).get(facing.axis)`.
        Method::Occlusion => {
            let mut plates = fence_gate_occlusion_boxes();
            if in_wall {
                // `SHAPE_OCCLUSION_WALL` is the mapped shape moved down 3/16 and
                // re-optimised; rotating a box and moving it along Y commute.
                plates = move_boxes(&plates, 0.0, -0.1875, 0.0);
            }
            union(&rotate_boxes(rotation, &plates))
        }
    }
}

/// `SHAPE_OCCLUSION` before rotation: the two side plates at the gate's edges.
fn fence_gate_occlusion_boxes() -> Vec<Box6> {
    vec![
        box16(0.0, 5.0, 7.0, 2.0, 16.0, 9.0),
        box16(14.0, 5.0, 7.0, 16.0, 16.0, 9.0),
    ]
}

// ---------------------------------------------------------------------------
// `CaveVinesBlock` / `CaveVinesPlantBlock`
// ---------------------------------------------------------------------------

/// `CaveVines.SHAPE = Block.column(14, 0, 16)`, the head and body shape; the
/// registered blocks collide with nothing, so collision and the default support
/// shape are empty while the occlusion shape is `getShape`.
fn cave_vines(method: Method) -> VoxelShape {
    match method {
        Method::Outline | Method::Occlusion => union(&[column16(14.0, 0.0, 16.0)]),
        Method::Collision | Method::Support => VoxelShape::empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state<'a>(pairs: &'a [(&'a str, &'a str)]) -> State<'a> {
        State::new(pairs)
    }

    /// A fence-gate state: facing, `open`, `in_wall`, unpowered.
    fn gate(
        facing: &'static str,
        open: &'static str,
        in_wall: &'static str,
    ) -> Vec<(&'static str, &'static str)> {
        vec![
            ("facing", facing),
            ("in_wall", in_wall),
            ("open", open),
            ("powered", "false"),
        ]
    }

    #[test]
    fn an_open_gate_only_keeps_its_occlusion_shape() {
        let open = gate("north", "true", "false");
        let state = state(&open);
        assert!(shape_for_family(Family::FenceGate, &state, Method::Collision).is_empty());
        assert!(shape_for_family(Family::FenceGate, &state, Method::Support).is_empty());
        assert!(!shape_for_family(Family::FenceGate, &state, Method::Occlusion).is_empty());
        assert!(!shape_for_family(Family::FenceGate, &state, Method::Outline).is_empty());
    }

    #[test]
    fn a_closed_gate_collides_like_a_twenty_four_sixteenth_column() {
        let closed = gate("north", "false", "false");
        let encoded =
            shape_for_family(Family::FenceGate, &state(&closed), Method::Collision).encode();
        assert_eq!(encoded.canonical, 0);
        assert_eq!((encoded.cube, encoded.cube_axes), (0, 0));
        assert_eq!(encoded.x, vec![0.0, 1.0]);
        assert_eq!(encoded.y, vec![0.0, 1.5]);
        assert_eq!(encoded.z, vec![0.375, 0.625]);
        assert_eq!(encoded.occupied, vec![1]);
        assert_eq!(encoded.discrete_max, [1, 1, 1]);
    }

    #[test]
    fn a_wall_gate_is_the_gate_clipped_below_thirteen_sixteenths() {
        let free = gate("north", "false", "false");
        let wall = gate("north", "false", "true");
        let free = shape_for_family(Family::FenceGate, &state(&free), Method::Outline).encode();
        let wall = shape_for_family(Family::FenceGate, &state(&wall), Method::Outline).encode();
        // `cube(16,16,4)` on the eighth grid: a 4/16 slab through the middle.
        assert_eq!((free.cube, free.cube_axes), (1, 7));
        assert_eq!(free.y, vec![0.0, 1.0]);
        assert_eq!(free.z.len() - 1, 8);
        assert_eq!(free.z[3], 0.375);
        assert_eq!(free.z[5], 0.625);
        // `ONLY_FIRST` with `column(16, 13, 16)` removes y >= 13/16.
        assert_eq!((wall.cube, wall.cube_axes), (0, 0));
        assert_eq!(wall.y, vec![0.0, 0.8125]);
        assert_eq!(wall.z, vec![0.375, 0.625]);
    }

    #[test]
    fn the_in_wall_occlusion_is_moved_down_three_sixteenths() {
        let free = gate("north", "false", "false");
        let wall = gate("north", "false", "true");
        let free = shape_for_family(Family::FenceGate, &state(&free), Method::Occlusion).encode();
        let wall = shape_for_family(Family::FenceGate, &state(&wall), Method::Occlusion).encode();
        assert_eq!(free.x, vec![0.0, 0.125, 0.875, 1.0]);
        assert_eq!(free.y, vec![0.3125, 1.0]);
        assert_eq!(free.occupied, vec![1, 0, 1]);
        assert_eq!(free.discrete_max, [3, 1, 1]);
        assert_eq!(wall.x, vec![0.0, 0.125, 0.875, 1.0]);
        assert_eq!(wall.y, vec![0.125, 0.8125]);
        assert_eq!(wall.occupied, vec![1, 0, 1]);
        assert_eq!(wall.discrete_max, [3, 1, 1]);
    }

    #[test]
    fn east_and_west_use_the_rotated_axis() {
        let north = gate("north", "false", "false");
        let east = gate("east", "false", "false");
        let north = shape_for_family(Family::FenceGate, &state(&north), Method::Occlusion).encode();
        let east = shape_for_family(Family::FenceGate, &state(&east), Method::Occlusion).encode();
        assert_eq!(east.x, vec![0.4375, 0.5625]);
        assert_eq!(east.y, north.y);
        assert_eq!(east.z, vec![0.0, 0.125, 0.875, 1.0]);
        assert_eq!(east.discrete_max, [1, 1, 3]);
        // The collision column rotates the same way: thick along X now.
        let east_collision = shape_for_family(
            Family::FenceGate,
            &state(&gate("east", "false", "false")),
            Method::Collision,
        )
        .encode();
        assert_eq!(east_collision.x, vec![0.375, 0.625]);
        assert_eq!(east_collision.y, vec![0.0, 1.5]);
        assert_eq!(east_collision.z, vec![0.0, 1.0]);
    }

    #[test]
    fn powered_never_changes_a_fence_gate_shape() {
        let unpowered = [
            ("facing", "south"),
            ("in_wall", "true"),
            ("open", "false"),
            ("powered", "false"),
        ];
        let powered = [
            ("facing", "south"),
            ("in_wall", "true"),
            ("open", "false"),
            ("powered", "true"),
        ];
        for method in [
            Method::Outline,
            Method::Collision,
            Method::Support,
            Method::Occlusion,
        ] {
            assert_eq!(
                shape_for_family(Family::FenceGate, &state(&unpowered), method),
                shape_for_family(Family::FenceGate, &state(&powered), method),
                "{method:?}"
            );
        }
    }

    #[test]
    fn cave_vines_are_a_thin_column_without_collision() {
        let berries = [("age", "7"), ("berries", "true")];
        let bare = [("age", "7"), ("berries", "false")];
        for pairs in [&berries, &bare] {
            let state = state(pairs);
            let outline = shape_for_family(Family::CaveVines, &state, Method::Outline).encode();
            assert_eq!(outline.canonical, 0);
            assert_eq!((outline.cube, outline.cube_axes), (0, 0));
            assert_eq!(outline.x, vec![0.0625, 0.9375]);
            assert_eq!(outline.y, vec![0.0, 1.0]);
            assert_eq!(outline.z, vec![0.0625, 0.9375]);
            assert_eq!(outline.occupied, vec![1]);
            assert_eq!(
                shape_for_family(Family::CaveVines, &state, Method::Occlusion),
                shape_for_family(Family::CaveVines, &state, Method::Outline)
            );
            assert!(shape_for_family(Family::CaveVines, &state, Method::Collision).is_empty());
            assert!(shape_for_family(Family::CaveVines, &state, Method::Support).is_empty());
        }
    }

    #[test]
    fn only_the_decoded_classes_are_covered() {
        assert_eq!(family_for_class("FenceGateBlock"), Some(Family::FenceGate));
        assert_eq!(family_for_class("CaveVinesBlock"), Some(Family::CaveVines));
        assert_eq!(
            family_for_class("CaveVinesPlantBlock"),
            Some(Family::CaveVines)
        );
        assert_eq!(family_for_class("WeepingVinesBlock"), None);
        assert_eq!(family_for_class("KelpBlock"), None);
        let empty: [(&str, &str); 0] = [];
        assert!(shape_for_class("WeepingVinesBlock", &state(&empty), Method::Occlusion).is_none());
    }
}
