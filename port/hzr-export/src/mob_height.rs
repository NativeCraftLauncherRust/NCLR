//! Correct `mob-data.json#states` heights, and the ZERO-level collision shapes
//! of the blocks whose shape cache is disabled.
//!
//! # Why `VoxelShape.max(Y)` is not the last coordinate
//!
//! The prepared tables store a `VoxelShape` as per-axis coordinate lists plus a
//! discrete occupancy grid (`(x * ySize + y) * zSize + z`). The shape engines
//! sometimes hand back a *repartitioned* shape: the coordinate list still spans
//! the whole unit interval while only the lower cells are occupied. For example
//! the exporter's repeater collision shape is
//!
//! ```text
//! y = [0, 1/16, ..., 1], occupied = [1, 0, 0, 0, 0, 0, 0, 0]
//! ```
//!
//! `VoxelShape.max(axis)` is `getCoords(axis).get(lastFull(axis) + 1)`, not the
//! last coordinate, and the exporter writes exactly that as the row height. A
//! plain `shape.y.last()` therefore reports 1.0 for the repeater (oracle 0.125),
//! for bottom halves of sensors, skulls, lightning rods, daylight detectors,
//! and so on. [`shape_height`] ports the occupancy-aware maximum. Slabs are the
//! same story in reverse: a top slab has `occupied = [0, 1]`, so its height is
//! `y[2] = 1.0`.
//!
//! # The dynamic blocks
//!
//! `BlockStateBase.initCache()` only builds the collision cache when
//! `!hasDynamicShape()`, which is why `cached-collision.nbt` stores `-1` for
//! 199 states. `VanillaMobDataExporter.states()` still calls
//! `state.getCollisionShape(EmptyBlockGetter.INSTANCE, BlockPos.ZERO)`, so the
//! block's own method answers for those states. Four classes have no shape in
//! the prepared tables and are covered here from their `client.jar` bytecode:
//!
//! * `BambooStalkBlock` — `SHAPE_COLLISION = Block.column(3, 0, 16)`
//!   (`x/z` 6.5/16..9.5/16, `y` 0..1; `isCollisionShapeFullBlock` is overridden
//!   to `false`);
//! * `ScaffoldingBlock` — with the empty `CollisionContext`,
//!   `isAbove(Shapes.block(), ZERO, true)` is `true` and `isDescending()` is
//!   `false`, so the collision answer is `SHAPE_STABLE`:
//!   `column(16, 14, 16)` (`x/z` full, `y` 14/16..1) unioned with the four
//!   horizontal rotations of `box(0, 0, 0, 2, 16, 2)` (corner posts, `y` 0..1);
//! * `PointedDripstoneBlock` / `SulfurSpikeBlock` — `SpeleothemBlock.getShape`
//!   by `THICKNESS`: `TIP_MERGE column(6,0,16)`, `TIP` + `TIP_DIRECTION=DOWN`
//!   `column(6,5,16)`, `TIP` + `UP` `column(6,0,11)`, `FRUSTUM column(8,0,16)`,
//!   `MIDDLE column(10,0,16)`, `BASE column(12,0,16)`. The `getOffset` move
//!   only shifts the shape in XZ (or not at all), so it does not affect the
//!   exported height.
//!
//! `ShulkerBoxBlock` answers `Shapes.block()`, `MovingPistonBlock` and
//! `PowderSnowBlock` answer `Shapes.empty()`; callers that already special-case
//! those keep working through [`collision_at_zero`].
//!
//! Include this module by path in `tests/mob_height.rs` (which also supplies
//! the `client.jar` path); the module path-includes `shape_final.rs` itself.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::Path;

#[path = "shape_final.rs"]
mod shape_final;

pub use shape_final::geometry::Shape;
use shape_final::block as blockmod;
use shape_final::geometry;
use shape_final::states as statemod;

// ---------------------------------------------------------------------------
// occupancy helpers
// ---------------------------------------------------------------------------

fn sizes(shape: &Shape) -> [usize; 3] {
    [
        shape.x.len().saturating_sub(1),
        shape.y.len().saturating_sub(1),
        shape.z.len().saturating_sub(1),
    ]
}

fn occupied(shape: &Shape, point: [usize; 3]) -> bool {
    let [xs, ys, zs] = sizes(shape);
    let index = (point[0] * ys + point[1]) * zs + point[2];
    let _ = xs;
    shape.occupied.get(index).copied().unwrap_or(0) != 0
}

/// `DiscreteVoxelShape.isEmpty()`.
pub fn shape_is_empty(shape: &Shape) -> bool {
    shape.occupied.iter().all(|value| *value == 0)
}

/// `VoxelShape.max(Direction.Axis)` for one axis index (0 = X, 1 = Y, 2 = Z):
/// `coords[lastFull(axis) + 1]`, or `None` for an empty shape.
pub fn shape_max(shape: &Shape, axis: usize) -> Option<f64> {
    if shape_is_empty(shape) {
        return None;
    }
    let [xs, ys, zs] = sizes(shape);
    let mut last = 0usize;
    for y in 0..ys {
        for z in 0..zs {
            for x in 0..xs {
                if !occupied(shape, [x, y, z]) {
                    continue;
                }
                let cell = match axis {
                    0 => x,
                    1 => y,
                    _ => z,
                };
                last = last.max(cell);
            }
        }
    }
    let coords = match axis {
        0 => &shape.x,
        1 => &shape.y,
        _ => &shape.z,
    };
    coords.get(last + 1).copied()
}

/// `VanillaMobDataExporter.states()`'s third column:
/// `shape.isEmpty() ? 0.0 : shape.max(Direction.Axis.Y)`.
pub fn shape_height(shape: &Shape) -> f64 {
    shape_max(shape, 1).unwrap_or(0.0)
}

/// A shape's plain field values, so an includer with its own copy of the
/// geometry module (`shape_final.rs` included at a different module path) can
/// bridge the two `Shape` types with a field copy.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShapeParts {
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    pub z: Vec<f64>,
    pub canonical: u8,
    pub cube: u8,
    pub cube_axes: u8,
    pub discrete_min: [i32; 3],
    pub discrete_max: [i32; 3],
    pub occupied: Vec<u8>,
}

impl ShapeParts {
    pub fn from_shape(shape: &Shape) -> ShapeParts {
        ShapeParts {
            x: shape.x.clone(),
            y: shape.y.clone(),
            z: shape.z.clone(),
            canonical: shape.canonical,
            cube: shape.cube,
            cube_axes: shape.cube_axes,
            discrete_min: shape.discrete_min,
            discrete_max: shape.discrete_max,
            occupied: shape.occupied.clone(),
        }
    }

    pub fn to_shape(&self) -> Shape {
        Shape {
            x: self.x.clone(),
            y: self.y.clone(),
            z: self.z.clone(),
            canonical: self.canonical,
            cube: self.cube,
            cube_axes: self.cube_axes,
            discrete_min: self.discrete_min,
            discrete_max: self.discrete_max,
            occupied: self.occupied.clone(),
        }
    }
}

/// `shape_height` for a shape that lives in another copy of the geometry
/// module (the includer passes the public fields).
pub fn height_from_parts(x: &[f64], y: &[f64], z: &[f64], occupied: &[u8]) -> f64 {
    shape_height(&Shape {
        x: x.to_vec(),
        y: y.to_vec(),
        z: z.to_vec(),
        canonical: 0,
        cube: 0,
        cube_axes: 0,
        discrete_min: [0, 0, 0],
        discrete_max: [0, 0, 0],
        occupied: occupied.to_vec(),
    })
}

/// `dynamic_shape` as plain fields, for the includer to fold into its own
/// `Shape` type.
pub fn dynamic_shape_parts(
    block_id: &str,
    values: &BTreeMap<String, String>,
) -> Option<ShapeParts> {
    dynamic_shape(block_id, values).map(|shape| ShapeParts::from_shape(&shape))
}

pub fn dynamic_shape_extra_parts(block_id: &str) -> Option<ShapeParts> {
    dynamic_shape_extra(block_id).map(|shape| ShapeParts::from_shape(&shape))
}

// ---------------------------------------------------------------------------
// ZERO-level dynamic collision shapes
// ---------------------------------------------------------------------------

fn column(width: f64, from: f64, to: f64) -> Shape {
    let half = width / 2.0;
    box_shape(
        8.0 - half,
        from,
        8.0 - half,
        8.0 + half,
        to,
        8.0 + half,
    )
}

/// `Block.box(x1, y1, z1, x2, y2, z2)` — pixel coordinates, divided by 16.
fn box_shape(x1: f64, y1: f64, z1: f64, x2: f64, y2: f64, z2: f64) -> Shape {
    Shape {
        x: vec![x1 / 16.0, x2 / 16.0],
        y: vec![y1 / 16.0, y2 / 16.0],
        z: vec![z1 / 16.0, z2 / 16.0],
        canonical: 0,
        cube: 0,
        cube_axes: 0,
        discrete_min: [0, 0, 0],
        discrete_max: [1, 1, 1],
        occupied: vec![1],
    }
}

fn union_shapes(shapes: &[Shape]) -> Shape {
    // `ScaffoldingBlock.SHAPE_STABLE` is the only union used here: the upper
    // slab plus the four corner posts. It is expressed with a shared grid so
    // the occupancy is exact.
    let mut xs = BTreeMap::new();
    let mut ys = BTreeMap::new();
    let mut zs = BTreeMap::new();
    for shape in shapes {
        for value in &shape.x {
            xs.insert(ordered(*value), ());
        }
        for value in &shape.y {
            ys.insert(ordered(*value), ());
        }
        for value in &shape.z {
            zs.insert(ordered(*value), ());
        }
    }
    let x: Vec<f64> = xs.into_keys().map(unordered).collect();
    let y: Vec<f64> = ys.into_keys().map(unordered).collect();
    let z: Vec<f64> = zs.into_keys().map(unordered).collect();
    let (xsize, ysize, zsize) = (
        x.len().saturating_sub(1),
        y.len().saturating_sub(1),
        z.len().saturating_sub(1),
    );
    let mut occupied = vec![0u8; xsize * ysize * zsize];
    for shape in shapes {
        for cell_x in 0..shape.x.len().saturating_sub(1) {
            for cell_y in 0..shape.y.len().saturating_sub(1) {
                for cell_z in 0..shape.z.len().saturating_sub(1) {
                    let index = (cell_x * (shape.y.len() - 1) + cell_y)
                        * (shape.z.len() - 1)
                        + cell_z;
                    if shape.occupied.get(index).copied().unwrap_or(0) == 0 {
                        continue;
                    }
                    let low = [shape.x[cell_x], shape.y[cell_y], shape.z[cell_z]];
                    let Some(rx) = x.iter().position(|value| *value == low[0]) else {
                        continue;
                    };
                    let Some(ry) = y.iter().position(|value| *value == low[1]) else {
                        continue;
                    };
                    let Some(rz) = z.iter().position(|value| *value == low[2]) else {
                        continue;
                    };
                    // Each cell is a single grid cell in the merged grid too.
                    occupied[(rx * ysize + ry) * zsize + rz] = 1;
                }
            }
        }
    }
    Shape {
        x,
        y,
        z,
        canonical: 0,
        cube: 0,
        cube_axes: 0,
        discrete_min: [0, 0, 0],
        discrete_max: [xsize as i32, ysize as i32, zsize as i32],
        occupied,
    }
}

fn ordered(value: f64) -> u64 {
    // Total order over the shape coordinates (all values are 0..1).
    value.to_bits()
}

fn unordered(bits: u64) -> f64 {
    f64::from_bits(bits)
}

fn empty_shape() -> Shape {
    geometry::Shape::empty()
}

fn block_shape() -> Shape {
    Shape {
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

fn scaffolding_stable() -> Shape {
    let slab = box_shape(0.0, 14.0, 0.0, 16.0, 16.0, 16.0);
    let posts = [
        box_shape(0.0, 0.0, 0.0, 2.0, 16.0, 2.0),
        box_shape(14.0, 0.0, 0.0, 16.0, 16.0, 2.0),
        box_shape(0.0, 0.0, 14.0, 2.0, 16.0, 16.0),
        box_shape(14.0, 0.0, 14.0, 16.0, 16.0, 16.0),
    ];
    let mut shapes = vec![slab];
    shapes.extend(posts.into_iter());
    union_shapes(&shapes)
}

/// The block's own `getCollisionShape(state, EmptyBlockGetter, ZERO,
/// CollisionContext.empty())` at ZERO for the four classes the prepared tables
/// leave uncached, or `None` for every other block.
pub fn dynamic_shape(block_id: &str, values: &BTreeMap<String, String>) -> Option<Shape> {
    match block_id {
        "minecraft:bamboo" => Some(column(3.0, 0.0, 16.0)),
        "minecraft:scaffolding" => Some(scaffolding_stable()),
        "minecraft:pointed_dripstone" | "minecraft:sulfur_spike" => {
            let thickness = values.get("thickness").map(String::as_str).unwrap_or("tip");
            Some(match thickness {
                "tip_merge" => column(6.0, 0.0, 16.0),
                "tip" => {
                    if values.get("vertical_direction").map(String::as_str) == Some("down") {
                        column(6.0, 5.0, 16.0)
                    } else {
                        column(6.0, 0.0, 11.0)
                    }
                }
                "frustum" => column(8.0, 0.0, 16.0),
                "middle" => column(10.0, 0.0, 16.0),
                _ => column(12.0, 0.0, 16.0),
            })
        }
        _ => None,
    }
}

/// The remaining three dynamic classes, which callers may already answer:
/// shulker boxes expose `Shapes.block()`, moving pistons and powder snow are
/// `Shapes.empty()`.
pub fn dynamic_shape_extra(block_id: &str) -> Option<Shape> {
    if block_id.contains("shulker_box") {
        Some(block_shape())
    } else if block_id == "minecraft:moving_piston" || block_id == "minecraft:powder_snow" {
        Some(empty_shape())
    } else {
        None
    }
}

/// `true` for the seven dynamic classes (the four above plus shulker boxes,
/// moving pistons and powder snow, which callers may already answer).
pub fn is_dynamic_block(block_id: &str) -> bool {
    matches!(
        block_id,
        "minecraft:bamboo"
            | "minecraft:scaffolding"
            | "minecraft:pointed_dripstone"
            | "minecraft:sulfur_spike"
            | "minecraft:moving_piston"
            | "minecraft:powder_snow"
    ) || block_id.contains("shulker_box")
}

/// The collision shape the exporter sees: the prepared-table entry when it
/// exists, otherwise the block's own answer at ZERO.
pub fn collision_at_zero(
    block_id: &str,
    values: &BTreeMap<String, String>,
    collision: Option<Shape>,
) -> Option<Shape> {
    collision
        .or_else(|| dynamic_shape(block_id, values))
        .or_else(|| dynamic_shape_extra(block_id))
}

// ---------------------------------------------------------------------------
// the whole height column
// ---------------------------------------------------------------------------

fn state_values(block: &statemod::BlockStates, offset: usize) -> BTreeMap<String, String> {
    let sorted = block.sorted();
    let counts: Vec<usize> = sorted.iter().map(|property| property.values.len()).collect();
    let mut out = BTreeMap::new();
    for (index, property) in sorted.iter().enumerate() {
        let mut product = 1usize;
        for count in counts.iter().skip(index + 1) {
            product = product.saturating_mul(*count);
        }
        if product == 0 {
            continue;
        }
        let value_index = (offset / product) % counts[index].max(1);
        if let Some(value) = property.values.get(value_index) {
            out.insert(property.name.clone(), value.clone());
        }
    }
    out
}

/// The `mob-data.json#states` height column, plus the blocks that could not be
/// answered (`collision` missing and not a known dynamic class).
pub fn heights(jar_path: &Path) -> Result<(Vec<f64>, Vec<String>), String> {
    let evaluated = shape_final::evaluate(jar_path)?;
    let registry = blockmod::registry(jar_path)?;
    let table = statemod::state_table(jar_path)?;
    let mut heights = Vec::with_capacity(table.state_count());
    let mut gaps = Vec::new();
    for (block_index, states) in table.blocks.iter().enumerate() {
        let block = &registry[block_index];
        for offset in 0..states.state_count {
            let index = heights.len();
            let collision = evaluated.collision.get(index).cloned().flatten();
            let values = state_values(states, offset);
            let shape = collision_at_zero(&block.id, &values, collision);
            match shape {
                Some(shape) => heights.push(shape_height(&shape)),
                None => {
                    heights.push(0.0);
                    if !gaps.contains(&block.id) {
                        gaps.push(block.id.clone());
                    }
                }
            }
        }
    }
    Ok((heights, gaps))
}

/// Write the height column as a JSON array (compact).
pub fn write_heights(jar_path: &Path, out: impl AsRef<Path>) -> Result<Vec<f64>, String> {
    let (heights, gaps) = heights(jar_path)?;
    if !gaps.is_empty() {
        return Err(format!("unresolved dynamic blocks: {gaps:?}"));
    }
    let text = serde_json::to_string(&heights).map_err(|error| error.to_string())?;
    let out = out.as_ref();
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    std::fs::write(out, text).map_err(|error| format!("{}: {error}", out.display()))?;
    Ok(heights)
}
