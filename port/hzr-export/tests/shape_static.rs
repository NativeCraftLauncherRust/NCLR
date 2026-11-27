//! End-to-end evidence for the extended shape-method VM.
//!
//! `src/shape_static.rs` runs the vanilla block classes' own bytecode (step 2 +
//! its static/registry/stream extensions); this test drives it on the player's
//! `client.jar` and compares the results with the prepared copies
//! (`tmp/ship-extract/cached-collision.nbt`, `occlusion.nbt` and
//! `worldgen/blocks/cached-support.nbt`), produced by the real game.
//!
//! Two layers of evidence:
//!
//! * **geometry equivalence** for *every* state the VM reports a shape for: the
//!   part of space both shapes occupy is identical (their coordinate
//!   partitions may differ, which step 1's `voxelshape.rs` owns);
//! * **byte-identical dictionaries** for the curated families whose factory
//!   calls this interpreter must reproduce exactly (single boxes, arrays,
//!   boolean/enum/int selectors, rotation maps and constructor-built
//!   `Function` shape fields).
//!
//! Current coverage (all 35,723 registry states driven through the VM):
//! collision 35,408/35,524 cached states covered, 25,784 byte-identical;
//! occlusion 34,644/35,723 covered, 20,648 byte-identical; support
//! 35,200/35,524 covered, 25,576 byte-identical. Everything covered is
//! point-set equal to the prepared table; the remainder is reported as gaps
//! (`None` plus the warning that caused it), never as a guessed shape.
//!
//! The remaining gaps are the block `offsetType`/`getOffset` shapes, the
//! `StateHolder.getValues()`-based multiface/vine families, and a handful of
//! class-initialiser builders (`ComposterBlock`, `DecoratedPotBlock`,
//! `CeilingHangingSignBlock`, `FenceGateBlock`, `HopperBlock`, ...); the module
//! docs of `shape_static.rs` list them by name.
#[path = "../src/block.rs"]
mod block;

#[path = "../src/geometry.rs"]
mod geometry;

#[path = "../src/shape_static.rs"]
mod shape_static;

#[path = "../src/states.rs"]
mod states;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use shape_static::{JoinOp, Query, Rotation, ShapeFactory, State, Vm};

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn prepared(name: &str) -> PathBuf {
    workspace().join("tmp/ship-extract").join(name)
}

// ---------------------------------------------------------------------------
// The reference shape model: the contract documented on `ShapeFactory`
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
struct RefShape {
    coords: [Vec<f64>; 3],
    occupied: BTreeSet<(usize, usize, usize)>,
    canonical: u8,
    cube: bool,
    cube_axes: u8,
}

impl RefShape {
    fn empty() -> RefShape {
        RefShape {
            coords: [vec![0.0], vec![0.0], vec![0.0]],
            occupied: BTreeSet::new(),
            canonical: 1,
            cube: false,
            cube_axes: 0,
        }
    }

    fn block() -> RefShape {
        RefShape {
            coords: [vec![0.0, 1.0], vec![0.0, 1.0], vec![0.0, 1.0]],
            occupied: BTreeSet::from([(0, 0, 0)]),
            canonical: 2,
            cube: true,
            cube_axes: 7,
        }
    }

    fn sizes(&self) -> [usize; 3] {
        [
            self.coords[0].len().saturating_sub(1),
            self.coords[1].len().saturating_sub(1),
            self.coords[2].len().saturating_sub(1),
        ]
    }

    fn is_empty(&self) -> bool {
        self.canonical == 1
    }

    /// Whether `point` is inside the shape. Used for the equivalence check and
    /// for rebuilding a merged shape.
    fn contains(&self, point: [f64; 3]) -> bool {
        let mut cell = [0usize; 3];
        for axis in 0..3 {
            let coords = &self.coords[axis];
            if point[axis] < coords[0] || point[axis] > *coords.last().unwrap() {
                return false;
            }
            let after = coords.partition_point(|value| *value <= point[axis]);
            if after == 0 || after > coords.len() - 1 {
                return false;
            }
            cell[axis] = after - 1;
        }
        self.occupied.contains(&(cell[0], cell[1], cell[2]))
    }

    /// The prepared dictionary's view of the shape.
    fn to_table_shape(&self) -> geometry::Shape {
        let sizes = self.sizes();
        let mut occupied = vec![0u8; sizes[0] * sizes[1] * sizes[2]];
        for (x, y, z) in &self.occupied {
            if *x < sizes[0] && *y < sizes[1] && *z < sizes[2] {
                occupied[(x * sizes[1] + y) * sizes[2] + z] = 1;
            }
        }
        let mut discrete_min = [0i32; 3];
        let mut discrete_max = [0i32; 3];
        if !self.occupied.is_empty() {
            // Each axis independently: `DiscreteVoxelShape.firstFull/lastFull`.
            let mut low = [usize::MAX; 3];
            let mut high = [0usize; 3];
            for (x, y, z) in &self.occupied {
                low[0] = low[0].min(*x);
                low[1] = low[1].min(*y);
                low[2] = low[2].min(*z);
                high[0] = high[0].max(*x + 1);
                high[1] = high[1].max(*y + 1);
                high[2] = high[2].max(*z + 1);
            }
            discrete_min = [low[0] as i32, low[1] as i32, low[2] as i32];
            discrete_max = [high[0] as i32, high[1] as i32, high[2] as i32];
        }
        geometry::Shape {
            x: self.coords[0].clone(),
            y: self.coords[1].clone(),
            z: self.coords[2].clone(),
            canonical: self.canonical,
            cube: if self.cube { 1 } else { 0 },
            cube_axes: self.cube_axes,
            discrete_min,
            discrete_max,
            occupied,
        }
    }
}

struct RefFactory;

impl RefFactory {
    fn find_bits(min: f64, max: f64) -> Option<i32> {
        if min < -1e-7 || max > 1.0 + 1e-7 {
            return None;
        }
        for bits in 0..=3i32 {
            let scale = (1i32 << bits) as f64;
            let low = min * scale;
            let high = max * scale;
            let low_ok = (low - low.round()).abs() < 1e-7 * scale;
            let high_ok = (high - high.round()).abs() < 1e-7 * scale;
            if low_ok && high_ok {
                return Some(bits);
            }
        }
        None
    }

    fn boxed(min: [f64; 3], max: [f64; 3]) -> RefShape {
        if (0..3).any(|axis| max[axis] - min[axis] < 1e-7) {
            return RefShape::empty();
        }
        let bits: Vec<Option<i32>> = (0..3)
            .map(|axis| Self::find_bits(min[axis], max[axis]))
            .collect();
        if bits.iter().all(|bits| *bits == Some(0)) {
            return RefShape::block();
        }
        if bits.iter().any(Option::is_none) {
            return RefShape {
                coords: [
                    vec![min[0], max[0]],
                    vec![min[1], max[1]],
                    vec![min[2], max[2]],
                ],
                occupied: BTreeSet::from([(0, 0, 0)]),
                canonical: 0,
                cube: false,
                cube_axes: 0,
            };
        }
        let sizes = [
            1usize << bits[0].unwrap(),
            1usize << bits[1].unwrap(),
            1usize << bits[2].unwrap(),
        ];
        let mut occupied = BTreeSet::new();
        let ranges: Vec<(usize, usize)> = (0..3)
            .map(|axis| {
                let low = (min[axis] * sizes[axis] as f64).round() as usize;
                let high = (max[axis] * sizes[axis] as f64).round() as usize;
                (low.min(sizes[axis]), high.min(sizes[axis]))
            })
            .collect();
        for x in ranges[0].0..ranges[0].1 {
            for y in ranges[1].0..ranges[1].1 {
                for z in ranges[2].0..ranges[2].1 {
                    occupied.insert((x, y, z));
                }
            }
        }
        let coords = [
            (0..=sizes[0]).map(|i| i as f64 / sizes[0] as f64).collect(),
            (0..=sizes[1]).map(|i| i as f64 / sizes[1] as f64).collect(),
            (0..=sizes[2]).map(|i| i as f64 / sizes[2] as f64).collect(),
        ];
        RefShape {
            coords,
            occupied,
            canonical: 0,
            cube: true,
            cube_axes: 7,
        }
    }

    fn merge(first: &RefShape, second: &RefShape, operation: JoinOp) -> RefShape {
        if operation == JoinOp::Or {
            if first.is_empty() {
                return second.clone();
            }
            if second.is_empty() {
                return first.clone();
            }
        }
        let mut coords: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for axis in 0..3 {
            let mut values: Vec<f64> = first.coords[axis]
                .iter()
                .chain(second.coords[axis].iter())
                .copied()
                .collect();
            values.sort_by(|a, b| a.partial_cmp(b).unwrap());
            values.dedup_by(|a, b| a.to_bits() == b.to_bits());
            coords[axis] = values;
        }
        let sizes = [
            coords[0].len() - 1,
            coords[1].len() - 1,
            coords[2].len() - 1,
        ];
        let mut occupied = BTreeSet::new();
        for x in 0..sizes[0] {
            for y in 0..sizes[1] {
                for z in 0..sizes[2] {
                    let point = [
                        (coords[0][x] + coords[0][x + 1]) / 2.0,
                        (coords[1][y] + coords[1][y + 1]) / 2.0,
                        (coords[2][z] + coords[2][z + 1]) / 2.0,
                    ];
                    let inside_first = first.contains(point);
                    let inside_second = second.contains(point);
                    if operation.apply(inside_first, inside_second) {
                        occupied.insert((x, y, z));
                    }
                }
            }
        }
        // Vanilla `optimize()` rebuilds the union from its boxes: a union whose
        // occupancy is a single cuboid becomes that one `Shapes.box` shape.
        if occupied.is_empty() {
            return RefShape::empty();
        }
        let mut low = [usize::MAX; 3];
        let mut high = [0usize; 3];
        for (x, y, z) in &occupied {
            low[0] = low[0].min(*x);
            low[1] = low[1].min(*y);
            low[2] = low[2].min(*z);
            high[0] = high[0].max(*x + 1);
            high[1] = high[1].max(*y + 1);
            high[2] = high[2].max(*z + 1);
        }
        let volume = (high[0] - low[0]) * (high[1] - low[1]) * (high[2] - low[2]);
        if occupied.len() == volume {
            let minimum = [coords[0][low[0]], coords[1][low[1]], coords[2][low[2]]];
            let maximum = [coords[0][high[0]], coords[1][high[1]], coords[2][high[2]]];
            return RefFactory::boxed(minimum, maximum);
        }
        RefShape {
            coords,
            occupied,
            canonical: 0,
            cube: false,
            cube_axes: 0,
        }
    }
}

impl ShapeFactory for RefFactory {
    type Shape = RefShape;

    fn empty(&mut self) -> RefShape {
        RefShape::empty()
    }

    fn block(&mut self) -> RefShape {
        RefShape::block()
    }

    fn create(&mut self, x1: f64, y1: f64, z1: f64, x2: f64, y2: f64, z2: f64) -> RefShape {
        RefFactory::boxed([x1, y1, z1], [x2, y2, z2])
    }

    fn or(&mut self, first: &RefShape, second: &RefShape) -> RefShape {
        RefFactory::merge(first, second, JoinOp::Or)
    }

    fn join(&mut self, first: &RefShape, second: &RefShape, operation: JoinOp) -> RefShape {
        RefFactory::merge(first, second, operation)
    }

    fn rotate(&mut self, shape: &RefShape, rotation: &Rotation, center: [f64; 3]) -> RefShape {
        if rotation.is_identity() {
            return shape.clone();
        }
        let sizes = shape.sizes();
        let mut coords: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for axis in 0..3 {
            let source = rotation.perm[axis];
            let from_center = center[source];
            let to_center = center[axis];
            let mut values: Vec<f64> = shape.coords[source]
                .iter()
                .map(|value| {
                    if rotation.inverted[axis] {
                        to_center - (*value - from_center)
                    } else {
                        *value - from_center + to_center
                    }
                })
                .collect();
            if rotation.inverted[axis] {
                values.reverse();
            }
            coords[axis] = values;
        }
        let mut occupied = BTreeSet::new();
        for (x, y, z) in &shape.occupied {
            let source = [*x, *y, *z];
            let mut cell = [0usize; 3];
            for axis in 0..3 {
                let index = source[rotation.perm[axis]];
                cell[axis] = if rotation.inverted[axis] {
                    sizes[rotation.perm[axis]] - 1 - index
                } else {
                    index
                };
            }
            occupied.insert((cell[0], cell[1], cell[2]));
        }
        RefShape {
            coords,
            occupied,
            canonical: 0,
            cube: shape.cube,
            cube_axes: if shape.cube { 7 } else { 0 },
        }
    }

    fn translate(&mut self, shape: &RefShape, delta: [f64; 3]) -> RefShape {
        if shape.is_empty() {
            return shape.clone();
        }
        RefShape {
            coords: [
                shape.coords[0].iter().map(|value| value + delta[0]).collect(),
                shape.coords[1].iter().map(|value| value + delta[1]).collect(),
                shape.coords[2].iter().map(|value| value + delta[2]).collect(),
            ],
            occupied: shape.occupied.clone(),
            canonical: 0,
            cube: false,
            cube_axes: 0,
        }
    }

    fn is_empty(&mut self, shape: &RefShape) -> bool {
        shape.is_empty()
    }
}

// ---------------------------------------------------------------------------
// The prepared data
// ---------------------------------------------------------------------------

/// One prepared block state, in registry (= table index) order.
#[derive(Clone, Debug)]
struct PreparedState {
    block_index: usize,
    block_id: String,
    class: String,
    values: BTreeMap<String, String>,
}

fn property_values(block: &states::BlockStates, index: usize) -> BTreeMap<String, String> {
    let sorted = block.sorted();
    let mut remainder = index;
    let mut values = BTreeMap::new();
    for property in sorted.iter().rev() {
        let count = property.values.len().max(1);
        let slot = remainder % count;
        remainder /= count;
        values.insert(property.name.clone(), property.values[slot].clone());
    }
    values
}

/// Resolve a block class simple name through the vanilla `block` tree.
fn resolve_class(names: &[String], simple: &str) -> Option<String> {
    let direct = format!("net/minecraft/world/level/block/{simple}");
    if names.iter().any(|name| *name == direct) {
        return Some(direct);
    }
    let suffix = format!("/{simple}");
    names
        .iter()
        .find(|name| name.starts_with("net/minecraft/world/level/block/") && name.ends_with(&suffix))
        .cloned()
}

fn prepared_states(names: &[String]) -> Vec<PreparedState> {
    let jar = jar();
    let registry = block::registry(&jar).expect("block registry");
    let table = states::state_table(&jar).expect("state table");
    assert_eq!(registry.len(), table.blocks.len(), "registry order");
    let mut out = Vec::new();
    for (index, row) in table.blocks.iter().enumerate() {
        assert_eq!(registry[index].id, row.id, "registry order at {index}");
        let class = resolve_class(names, &registry[index].class)
            .unwrap_or_else(|| panic!("class {} not in the jar", registry[index].class));
        for state_index in 0..row.state_count {
            out.push(PreparedState {
                block_index: index,
                block_id: row.id.clone(),
                class: class.clone(),
                values: property_values(row, state_index),
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------

/// Point-set equality: both shapes occupy the same part of the block.
fn equivalent(actual: &RefShape, expected: &geometry::Shape) -> bool {
    let mut coords: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for axis in 0..3 {
        let expected_coords = match axis {
            0 => &expected.x,
            1 => &expected.y,
            _ => &expected.z,
        };
        let mut values: Vec<f64> = expected_coords
            .iter()
            .chain(actual.coords[axis].iter())
            .copied()
            .collect();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        values.dedup_by(|a, b| a.to_bits() == b.to_bits());
        coords[axis] = values;
    }
    let sizes = [
        coords[0].len() - 1,
        coords[1].len() - 1,
        coords[2].len() - 1,
    ];
    let expected_sizes = [
        expected.x.len().saturating_sub(1),
        expected.y.len().saturating_sub(1),
        expected.z.len().saturating_sub(1),
    ];
    let expected_contains = |point: [f64; 3]| -> bool {
        let mut cell = [0usize; 3];
        for axis in 0..3 {
            let values = match axis {
                0 => &expected.x,
                1 => &expected.y,
                _ => &expected.z,
            };
            let after = values.partition_point(|value| *value <= point[axis]);
            if after == 0 || after > values.len() - 1 {
                return false;
            }
            cell[axis] = after - 1;
        }
        if cell[0] >= expected_sizes[0] || cell[1] >= expected_sizes[1] || cell[2] >= expected_sizes[2]
        {
            return false;
        }
        expected.occupied[(cell[0] * expected_sizes[1] + cell[1]) * expected_sizes[2] + cell[2]] != 0
    };
    for x in 0..sizes[0] {
        for y in 0..sizes[1] {
            for z in 0..sizes[2] {
                let point = [
                    (coords[0][x] + coords[0][x + 1]) / 2.0,
                    (coords[1][y] + coords[1][y + 1]) / 2.0,
                    (coords[2][z] + coords[2][z + 1]) / 2.0,
                ];
                if actual.contains(point) != expected_contains(point) {
                    return false;
                }
            }
        }
    }
    true
}

fn mismatch(actual: &RefShape, expected: &geometry::Shape) -> String {
    let produced = actual.to_table_shape();
    let fields = [
        ("canonical", produced.canonical as i32, expected.canonical as i32),
        ("cube", produced.cube as i32, expected.cube as i32),
        ("cube_axes", produced.cube_axes as i32, expected.cube_axes as i32),
        ("x", produced.x.len() as i32, expected.x.len() as i32),
        ("y", produced.y.len() as i32, expected.y.len() as i32),
        ("z", produced.z.len() as i32, expected.z.len() as i32),
    ];
    let mut parts = Vec::new();
    for (name, got, want) in fields {
        if got != want {
            parts.push(format!("{name} got {got} want {want}"));
        }
    }
    if produced.x != expected.x {
        parts.push(format!("x {:?} != {:?}", produced.x, expected.x));
    }
    if produced.y != expected.y {
        parts.push(format!("y {:?} != {:?}", produced.y, expected.y));
    }
    if produced.z != expected.z {
        parts.push(format!("z {:?} != {:?}", produced.z, expected.z));
    }
    if produced.discrete_min != expected.discrete_min {
        parts.push(format!(
            "discrete_min {:?} != {:?}",
            produced.discrete_min, expected.discrete_min
        ));
    }
    if produced.discrete_max != expected.discrete_max {
        parts.push(format!(
            "discrete_max {:?} != {:?}",
            produced.discrete_max, expected.discrete_max
        ));
    }
    if produced.occupied != expected.occupied {
        parts.push("occupancy differs".to_string());
    }
    if parts.is_empty() {
        "coordinate bit patterns differ".to_string()
    } else {
        parts.join("; ")
    }
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

/// The curated families whose factory calls this interpreter must reproduce
/// byte for byte: single boxes, arrays, `int`/boolean/enum selectors, super
/// calls and the rotation maps.
const CURATED: [&str; 11] = [
    "SlabBlock",
    "SnowLayerBlock",
    "LadderBlock",
    "ChestBlock",
    "CactusBlock",
    "HeavyCoreBlock",
    "PathBlock",
    "MudBlock",
    "LeavesBlock",
    "ChainBlock",
    "AttachedStemBlock",
];

/// Families whose *geometry* is checked but whose byte layout is not: their
/// shapes are `Shapes.or(...)` compositions, and vanilla's `optimize()` rebuilds
/// the box list (merging AABBs) before caching, which is the reference
/// factory's step-1 responsibility, not the interpreter's. The interpreter is
/// proven correct here by the point-set comparison.
const PARTITION_ONLY: [&str; 3] = ["StairBlock", "LanternBlock", "AnvilBlock"];

#[test]
fn the_interpreter_reproduces_the_prepared_shape_tables() {
    if !jar().exists() || !prepared("cached-collision.nbt").exists() {
        eprintln!("skipped: needs mc263/client.jar and tmp/ship-extract");
        return;
    }
    let source = shape_static::JarSource::open(&jar()).expect("jar");
    let names: Vec<String> = source.names().map(str::to_string).collect();
    let mut vm = Vm::new(Box::new(source), RefFactory);
    let states = prepared_states(&names);
    let block_count = states.iter().map(|state| state.block_index).max().unwrap_or(0) + 1;

    for (table_name, query) in [
        ("cached-collision.nbt", Query::Collision),
        ("occlusion.nbt", Query::Occlusion),
        ("worldgen/blocks/cached-support.nbt", Query::BlockSupport),
    ] {
        let table = geometry::read_table(&prepared(table_name)).expect("prepared table");
        assert_eq!(table.cached.len(), states.len(), "{table_name}: one per state");
        let cached_states = table.cached.iter().filter(|slot| **slot >= 0).count();

        // `Properties.noCollision()` is a registration fact, not shape-method
        // bytecode, so it is this test's input. Derive it from the table: a
        // block whose every cached shape in this table is empty is a
        // non-colliding block; for every other block `has_collision` is true,
        // and *all* of its states (including empty ones) are then validated
        // against the VM. Empty-only blocks are therefore not evidence.
        let mut collides = vec![false; block_count];
        for (index, state) in states.iter().enumerate() {
            let slot = table.cached[index];
            if slot >= 0 && table.shapes[slot as usize].canonical != 1 {
                collides[state.block_index] = true;
            }
        }

        let mut covered = 0usize;
        let mut exact = 0usize;
        let mut geometry_only = 0usize;
        let mut partition_only_covered: BTreeMap<String, usize> = BTreeMap::new();
        let mut gaps: BTreeMap<String, (usize, String)> = BTreeMap::new();
        let mut failures: Vec<String> = Vec::new();
        let mut curated_mismatch: BTreeMap<String, String> = BTreeMap::new();

        for (index, state) in states.iter().enumerate() {
            let dictionary_index = table.cached[index];
            if dictionary_index < 0 {
                continue;
            }
            let expected = &table.shapes[dictionary_index as usize];
            let state = State {
                class: state.class.clone(),
                block: state.block_id.clone(),
                values: state.values.clone(),
                has_collision: collides[state.block_index],
            };
            let result = vm.query(&state, query);
            let Some(actual) = result.shape else {
                let entry = gaps
                    .entry(state.class.clone())
                    .or_insert_with(|| (0, String::new()));
                entry.0 += 1;
                if entry.1.is_empty() {
                    entry.1 = result
                        .warnings
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "no shape".to_string());
                }
                continue;
            };
            covered += 1;
            let equivalent = equivalent(&actual, expected);
            let identical = actual.to_table_shape() == *expected;
            if identical {
                exact += 1;
            } else if equivalent {
                geometry_only += 1;
            }
            if !equivalent {
                failures.push(format!(
                    "{table_name} state {index} {} {:?}: {}",
                    state.class,
                    state.values,
                    mismatch(&actual, expected)
                ));
                if failures.len() > 10 {
                    failures.push(format!(
                        "{table_name}: (more mismatches suppressed)"
                    ));
                }
            }
            let simple = state.class.rsplit('/').next().unwrap_or_default();
            if PARTITION_ONLY.contains(&simple) {
                *partition_only_covered
                    .entry(simple.to_string())
                    .or_default() += 1;
            }
            if CURATED.contains(&simple) && !identical {
                curated_mismatch
                    .entry(simple.to_string())
                    .or_insert_with(|| mismatch(&actual, expected));
            }
        }

        eprintln!(
            "{table_name}: cached {cached_states}, covered {covered}, exact {exact}, geometry-equal-but-repartitioned {geometry_only}, gap classes {}",
            gaps.len()
        );
        for (class, (count, warning)) in &gaps {
            eprintln!("  gap {class}: {count} states ({warning})");
        }
        assert!(
            failures.is_empty(),
            "{table_name}: interpreted shapes disagree with the prepared table:\n{}",
            failures.join("\n")
        );
        assert!(
            covered >= 3_000,
            "{table_name}: coverage regressed to {covered} states"
        );
        assert!(
            exact * 100 / covered.max(1) >= 50,
            "{table_name}: only {exact}/{covered} states are byte-identical"
        );
        assert!(
            curated_mismatch.is_empty(),
            "curated families are not byte-identical:\n{curated_mismatch:#?}"
        );
        for family in PARTITION_ONLY {
            assert!(
                partition_only_covered.contains_key(family),
                "{table_name}: {family} produced no shape at all"
            );
        }
    }
}
#[test]
fn the_decoder_reads_a_tableswitch() {
    // tableswitch at offset 0 with low 0, high 1, default +20, targets +4/+8.
    let mut code = vec![0xaa, 0, 0, 0];
    code.extend_from_slice(&20i32.to_be_bytes());
    code.extend_from_slice(&0i32.to_be_bytes());
    code.extend_from_slice(&1i32.to_be_bytes());
    code.extend_from_slice(&4i32.to_be_bytes());
    code.extend_from_slice(&8i32.to_be_bytes());
    // offsets 24..: nop, nop, return, nop
    code.extend_from_slice(&[0x00, 0x00, 0xb1, 0x00]);
    let decoded = shape_static::decode_code(&code);
    assert_eq!(decoded[0].opcode, 0xaa);
    let switch = decoded[0].switch.as_ref().expect("switch payload");
    assert_eq!(switch.default, 20);
    assert_eq!(switch.keys, vec![0, 1]);
    assert_eq!(switch.targets, vec![4, 8]);
}
