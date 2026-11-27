//! Do the per-class shape selectors reproduce Horizon's prepared shape tables?
//!
//! The reference is `tmp/ship-extract`: `cached-collision.nbt` and
//! `occlusion.nbt`, indexed by state id, plus `_shared/states.bin` for the block
//! id of every state. The state -> property map is rebuilt here from the jar
//! (`states::state_table`), so a selector is checked against every state of every
//! covered block, not against hand-picked examples.
//!
//! Every covered state must match exactly: coordinates, occupancy, discrete
//! bounds and the `canonical`/`cube`/`cube_axes` class flags.

#[path = "../src/block.rs"]
mod block;
#[path = "../src/geometry.rs"]
mod geometry;
#[path = "../src/shape_families.rs"]
mod shape_families;
#[path = "../src/states.rs"]
mod states;

use std::collections::BTreeMap;
use std::path::PathBuf;

use shape_families::{shape_for_class, ShapeKind, State};

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn reference(name: &str) -> PathBuf {
    workspace().join("tmp/ship-extract").join(name)
}

/// One state in registry order.
struct Row {
    block: String,
    class: String,
    properties: Vec<(String, String)>,
}

/// `Lists.cartesianProduct` in `StateDefinition` order: sorted property names,
/// first property varies slowest.
fn cartesian(properties: &[states::Property]) -> Vec<Vec<(String, String)>> {
    let mut out: Vec<Vec<(String, String)>> = vec![Vec::new()];
    for property in properties {
        let mut next = Vec::with_capacity(out.len() * property.values.len());
        for prefix in &out {
            for value in &property.values {
                let mut row = prefix.clone();
                row.push((property.name.clone(), value.clone()));
                next.push(row);
            }
        }
        out = next;
    }
    out
}

/// Every state of every block, in registry order, with its class and properties.
fn state_rows() -> Vec<Row> {
    let table = states::state_table(&jar()).expect("the state table");
    let registry = block::registry(&jar()).expect("the block registry");
    assert_eq!(table.blocks.len(), registry.len(), "block count");
    let mut rows = Vec::with_capacity(table.state_count());
    for (block, registered) in table.blocks.iter().zip(registry.iter()) {
        assert_eq!(block.id, registered.id, "registry order");
        let sorted = block.sorted();
        let combos = cartesian(&sorted.iter().map(|p| (*p).clone()).collect::<Vec<_>>());
        assert_eq!(combos.len(), block.state_count, "{} state count", block.id);
        for combo in combos {
            rows.push(Row {
                block: block.id.clone(),
                class: registered.class.clone(),
                properties: combo,
            });
        }
    }
    rows
}

fn pairs(row: &Row) -> Vec<(&str, &str)> {
    row.properties
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

fn same(encoded: &shape_families::EncodedShape, theirs: &geometry::Shape) -> Option<String> {
    let mut diffs = Vec::new();
    if encoded.x != theirs.x {
        diffs.push(format!("x {:?} != {:?}", encoded.x, theirs.x));
    }
    if encoded.y != theirs.y {
        diffs.push(format!("y {:?} != {:?}", encoded.y, theirs.y));
    }
    if encoded.z != theirs.z {
        diffs.push(format!("z {:?} != {:?}", encoded.z, theirs.z));
    }
    if encoded.canonical != theirs.canonical {
        diffs.push(format!(
            "canonical {} != {}",
            encoded.canonical, theirs.canonical
        ));
    }
    if encoded.cube != theirs.cube {
        diffs.push(format!("cube {} != {}", encoded.cube, theirs.cube));
    }
    if encoded.cube_axes != theirs.cube_axes {
        diffs.push(format!(
            "cube_axes {} != {}",
            encoded.cube_axes, theirs.cube_axes
        ));
    }
    if encoded.discrete_min != theirs.discrete_min {
        diffs.push(format!(
            "discrete_min {:?} != {:?}",
            encoded.discrete_min, theirs.discrete_min
        ));
    }
    if encoded.discrete_max != theirs.discrete_max {
        diffs.push(format!(
            "discrete_max {:?} != {:?}",
            encoded.discrete_max, theirs.discrete_max
        ));
    }
    if encoded.occupied != theirs.occupied {
        diffs.push(format!(
            "occupied ({} cells) != ({} cells)",
            encoded.occupied.len(),
            theirs.occupied.len()
        ));
    }
    if diffs.is_empty() {
        None
    } else {
        Some(diffs.join(", "))
    }
}

#[derive(Default)]
struct Stats {
    blocks: BTreeMap<String, usize>,
    states: usize,
    compared: usize,
}

fn family_name(family: shape_families::Family) -> String {
    format!("{family:?}")
}

#[test]
fn every_state_has_a_reference_shape() {
    if !jar().exists() || !reference(geometry::COLLISION).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let rows = state_rows();
    let states = geometry::state_blocks(&workspace().join("tmp/ship-extract/_shared/states.bin"))
        .expect("the prepared states");
    assert_eq!(rows.len(), states.len(), "state count");
    let collision = geometry::read_table(&reference(geometry::COLLISION)).expect("collision");
    let occlusion = geometry::read_table(&reference(geometry::OCCLUSION)).expect("occlusion");
    assert_eq!(collision.cached.len(), rows.len());
    assert_eq!(occlusion.cached.len(), rows.len());
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(
            states[index].0, row.block,
            "state {index} belongs to another block"
        );
    }
}

#[test]
fn the_state_enumeration_matches_known_defaults() {
    if !jar().exists() || !reference(geometry::COLLISION).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let rows = state_rows();
    let start = |id: &str| {
        rows.iter()
            .position(|row| row.block == id)
            .unwrap_or_else(|| panic!("{id} is registered"))
    };
    let shape = |index: usize| {
        let collision =
            geometry::read_table(&reference(geometry::COLLISION)).expect("collision");
        collision.shapes[collision.cached[index] as usize].clone()
    };

    // Oak slab: states are (type, waterlogged), type fastest-varying-last so
    // offsets 0 and 2 are top and bottom.
    let oak_slab = start("minecraft:oak_slab");
    let props = |index: usize, name: &str| {
        rows[index]
            .properties
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    assert_eq!(props(oak_slab, "type").as_deref(), Some("top"));
    assert_eq!(props(oak_slab + 2, "type").as_deref(), Some("bottom"));
    assert_eq!(props(oak_slab + 4, "type").as_deref(), Some("double"));
    assert_eq!(shape(oak_slab).y, vec![0.0, 0.5, 1.0]);
    assert_eq!(shape(oak_slab + 2).y, vec![0.0, 0.5, 1.0]);
    assert_eq!(shape(oak_slab + 4).canonical, 2);

    // Oak stairs: the prepared default state id is 5474; the block starts at
    // 5463, so offset 11 must be north / bottom / straight / not waterlogged.
    let oak_stairs = start("minecraft:oak_stairs");
    let default = oak_stairs + 11;
    assert_eq!(props(default, "facing").as_deref(), Some("north"));
    assert_eq!(props(default, "half").as_deref(), Some("bottom"));
    assert_eq!(props(default, "shape").as_deref(), Some("straight"));
    assert_eq!(props(default, "waterlogged").as_deref(), Some("false"));

    // Oak fence default (id 8666) has no connections at all.
    let oak_fence = start("minecraft:oak_fence");
    let default = oak_fence + 31;
    for direction in ["north", "east", "south", "west"] {
        assert_eq!(props(default, direction).as_deref(), Some("false"));
    }
}

#[test]
fn every_covered_state_matches_the_prepared_tables() {
    if !jar().exists() || !reference(geometry::COLLISION).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let rows = state_rows();
    let collision = geometry::read_table(&reference(geometry::COLLISION)).expect("collision");
    let occlusion = geometry::read_table(&reference(geometry::OCCLUSION)).expect("occlusion");
    let mut stats: BTreeMap<String, Stats> = BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();
    let mut uncached = 0usize;
    for (index, row) in rows.iter().enumerate() {
        let values = pairs(row);
        let state = State::new(&values);
        for (kind, table) in [
            (ShapeKind::Collision, &collision),
            (ShapeKind::Occlusion, &occlusion),
        ] {
            let Some(shape) = shape_for_class(&row.class, &state, kind) else {
                continue;
            };
            // The collision oracle leaves states it never queried at -1; those
            // cannot be checked, but the occlusion table always has a shape.
            if table.cached[index] < 0 {
                uncached += 1;
                continue;
            }
            if let Some(family) = shape_families::family_for_class(&row.class) {
                let entry = stats.entry(family_name(family)).or_default();
                entry.states += 1;
                entry
                    .blocks
                    .insert(row.block.clone(), entry.blocks.len());
            }
            let encoded = shape.encode();
            let theirs = &table.shapes[table.cached[index] as usize];
            if let Some(diff) = same(&encoded, theirs) {
                if failures.len() < 40 {
                    failures.push(format!(
                        "state {index} {} {kind:?} {}: {diff}",
                        row.block,
                        format_properties(&row.properties)
                    ));
                }
            } else if let Some(family) = shape_families::family_for_class(&row.class) {
                stats
                    .get_mut(&family_name(family))
                    .expect("family entry")
                    .compared += 1;
            }
        }
    }
    let mut summary = String::new();
    for (family, entry) in &stats {
        summary.push_str(&format!(
            "  {family}: {} blocks, {} state-shapes checked ({})\n",
            entry.blocks.len(),
            entry.compared,
            entry.states
        ));
    }
    eprintln!("shape_families coverage:\n{summary}");
    eprintln!("  uncached collision states (reference writes -1): {uncached}");
    assert!(
        failures.is_empty(),
        "{} covered shapes differ; first differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn the_uncovered_non_default_classes_are_reported_as_gaps() {
    if !jar().exists() || !reference(geometry::COLLISION).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let rows = state_rows();
    let collision = geometry::read_table(&reference(geometry::COLLISION)).expect("collision");
    let occlusion = geometry::read_table(&reference(geometry::OCCLUSION)).expect("occlusion");
    // A class needs a selector when a state's shape is neither the empty nor the
    // full-block default (canonical != 0). Classes this module handles are not
    // gaps; everything else is listed so the next engine step knows its work.
    let mut gaps: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        if shape_families::family_for_class(&row.class).is_some() {
            continue;
        }
        let collision_interesting = collision.cached[index] >= 0
            && collision.shapes[collision.cached[index] as usize].canonical == 0;
        let occlusion_interesting =
            occlusion.shapes[occlusion.cached[index] as usize].canonical == 0;
        let interesting = collision_interesting || occlusion_interesting;
        if !interesting {
            continue;
        }
        let entry = gaps.entry(row.class.clone()).or_insert((0, 0, 0));
        entry.0 += 1;
        if entry.1 == 0 {
            entry.1 = index;
        }
        entry.2 = index;
    }
    let mut report = String::new();
    let mut total = 0usize;
    for (class, (count, _, _)) in &gaps {
        total += count;
        report.push_str(&format!("  {class}: {count} states\n"));
    }
    eprintln!(
        "classes needing a shape selector but not covered here ({} states over {} classes):\n{report}",
        total,
        gaps.len()
    );
    // The selector's families are the big geometric families; the rest (fence
    // gates, beds, mushrooms, ...) stay on the gap list for the VM step.
    assert!(
        shape_families::CLASSES.len() >= 30,
        "the covered class list should name the families"
    );
}

fn format_properties(properties: &[(String, String)]) -> String {
    properties
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join(",")
}






#[test]
fn leaves_are_the_default_full_block_path_not_a_selector() {
    if !jar().exists() || !reference(geometry::COLLISION).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let rows = state_rows();
    let collision = geometry::read_table(&reference(geometry::COLLISION)).expect("collision");
    let occlusion = geometry::read_table(&reference(geometry::OCCLUSION)).expect("occlusion");
    let mut leaves = 0usize;
    for (index, row) in rows.iter().enumerate() {
        if row.class != "LeavesBlock" {
            continue;
        }
        leaves += 1;
        // No shape-method override on the class chain: the default path owns
        // it, and the prepared tables are `Shapes.block()` (canonical 2).
        assert_eq!(
            shape_families::family_for_class(&row.class),
            None,
            "{} must stay on the default path",
            row.block
        );
        assert_eq!(collision.shapes[collision.cached[index] as usize].canonical, 2);
        assert_eq!(
            occlusion.shapes[occlusion.cached[index] as usize].canonical,
            2
        );
    }
    assert!(leaves > 0, "the jar has leaves");
}
