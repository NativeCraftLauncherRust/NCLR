//! The VoxelShape VM the remaining collision/occlusion/support states need.
//!
//! This module is the design + invariant layer for the geometry VM that
//! `geometry.rs` currently leaves as gaps: the 841 block classes that override
//! `getShape`/`getCollisionShape`/`getFaceOcclusionShape`/`isFaceSturdy`, and the
//! `full_block` predicate for `structure-blocks.json`.
//!
//! # What the prepared files store
//!
//! The shape dictionary entries in `cached-collision.nbt`, `occlusion.nbt` and
//! `worldgen/blocks/cached-support.nbt` come from `VanillaCollisionOracle`'s
//! `grid(VoxelShape)`, which reads the live shape object:
//!
//! * `x`/`y`/`z` — `VoxelShape.getCoords(axis)`: the sorted, de-duplicated
//!   boundary coordinates of the union of boxes, as raw `double` bits
//!   (normalized 0..1; vanilla boxes are aligned to 1/16, so 6/16, 15/16, ...);
//! * `discrete_min`/`discrete_max` — `DiscreteVoxelShape.firstFull(axis)` and
//!   `lastFull(axis)`: the first occupied cell and **one past** the last one;
//! * `occupied` — the discrete occupancy over the grid
//!   `(len(x)-1) * (len(y)-1) * (len(z)-1)` cells, x-major then y then z;
//! * `canonical` — `1` when the object is `Shapes.empty()`, `2` when it is
//!   `Shapes.block()`, else `0`;
//! * `cube` — `1` when the object is a `CubeVoxelShape`;
//! * `cube_axes` — bit `axis.ordinal()` when `getCoords(axis)` is a
//!   `CubePointRange`.
//!
//! Every one of those fields is checked by [`tests::the_dictionary_invariants_hold`]
//! on all 19,452 dictionary entries of the three prepared tables.
//!
//! # Why the VM is not just "union of boxes"
//!
//! `canonical`, `cube` and `cube_axes` are **object identity / class** facts, not
//! geometry: a union that covers the full cube but was built as an
//! `ArrayVoxelShape` prints `canonical 0, cube 0`, while the same geometry built
//! through `Shapes.block()` prints `2, 1, 7`. The prepared copy also shows that
//! `CubeVoxelShape` is not only the full cube (half slabs carry `cube 1`,
//! `cube_axes 7`), so the VM has to reproduce vanilla's *construction* path, not
//! only the resulting box list:
//!
//! * `Shapes.box`, `Shapes.column`, `Shapes.create`, `Shapes.empty`,
//!   `Shapes.block`, `Shapes.or`, `Shapes.join`, `Block.box`, `Block.column`,
//!   `Block.cube` and their dedup/`CubeVoxelShape` selection rules;
//! * `VoxelShape` discretization (`DiscreteVoxelShape` cell grid, `firstFull` /
//!   `lastFull`), and `getCoords` behaviour (`CubePointRange` vs array).
//!
//! # The VM plan
//!
//! 1. **Shape constructors** — a `VoxelShape` model carrying the same fields the
//!    oracle reads, with the vanilla class-selection rules for the factories
//!    above. This is the piece that must be exact before any block method is
//!    interpreted.
//! 2. **Method interpreter** — evaluate the overriding method in the block's
//!    class chain: `super` calls, `state.getValue(property)` for enum/boolean/
//!    integer properties, `switch`/`tableswitch` on enum ordinals, cached shape
//!    fields built in `<clinit>` (`Map<Direction,VoxelShape>`, arrays indexed by
//!    property index), and `Shapes.or`/`join` composition of the cached values.
//! 3. **Queries** — collision (`getCollisionShape` → `getShape` default),
//!    occlusion (`getFaceOcclusionShape` + `getOcclusionShape`), support
//!    (`isFaceSturdy`/`getBlockSupportShape`, which is per-face and measured as
//!    *not* geometry-equivalent to the block shape), and `full_block`
//!    (`isCollisionShapeFullBlock`).
//!
//! Each step produces a `Coverage`-style report; states a step cannot derive are
//! left `None` rather than guessed, exactly like `geometry::collision_shapes`.
//!
//! # Current status
//!
//! The container codec in `geometry.rs` is complete and byte-identical for all
//! three files (round-trip tested). The default-path subset (3,151 states) is
//! exact. The VM above is **not implemented yet**: it needs the vanilla
//! `VoxelShape` construction semantics reproduced first (step 1), which is the
//! hard prerequisite for steps 2–3.

/// The shape tables this VM would fill.
pub const TARGETS: [(&str, &str); 3] = [
    (
        "cached-collision.nbt",
        "getCollisionShape(state, EmptyBlockGetter, BlockPos.ZERO, CollisionContext.empty())",
    ),
    (
        "occlusion.nbt",
        "getFaceOcclusionShape / getOcclusionShape for all six directions",
    ),
    (
        "worldgen/blocks/cached-support.nbt",
        "isFaceSturdy / getBlockSupportShape per face (kind: block_support)",
    ),
];

/// The per-shape invariants the dictionary obeys, exposed for the VM's tests.
///
/// `(len_x - 1) * (len_y - 1) * (len_z - 1)` is the occupancy array length of a
/// non-empty entry; an empty entry (`canonical 1`) has no occupancy.
pub fn occupancy_cells(x: usize, y: usize, z: usize, canonical: u8) -> usize {
    if canonical == 1 {
        0
    } else {
        x.saturating_sub(1) * y.saturating_sub(1) * z.saturating_sub(1)
    }
}
