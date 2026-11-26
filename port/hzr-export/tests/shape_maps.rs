//! Do the static-map shape builders reproduce Horizon's prepared shape tables?
//!
//! The reference is `tmp/ship-extract`: `cached-collision.nbt` and
//! `occlusion.nbt`, indexed by state id, plus `_shared/states.bin` for the block
//! id of every state. The state -> property map is rebuilt here from the jar
//! (`states::state_table`), so every state of every covered block is checked,
//! not a hand-picked default state.
//!
//! Coverage is the seven classes `shape_maps` documents: the ceiling hanging
//! signs, the flower beds, leaf litter, the composter, the decorated pot, the
//! hopper and the mossy carpet. Every covered state must match byte for byte:
//! coordinates, occupancy, discrete bounds and the `canonical`/`cube`/
//! `cube_axes` class flags, in both tables.

#[path = "../src/block.rs"]
mod block;
#[path = "../src/geometry.rs"]
mod geometry;
#[path = "../src/shape_maps.rs"]
mod shape_maps;
#[path = "../src/states.rs"]
mod states;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use shape_maps::{shape_for_class, MapFamily, ShapeKind, State};

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

fn format_properties(properties: &[(String, String)]) -> String {
    properties
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn same(encoded: &shape_maps::EncodedShape, theirs: &geometry::Shape) -> Option<String> {
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

fn family_name(family: MapFamily) -> String {
    format!("{family:?}")
}

#[derive(Default)]
struct Stats {
    blocks: BTreeSet<String>,
    states: usize,
    compared: usize,
}

#[test]
fn the_covered_classes_are_the_static_map_builders() {
    let mut names: Vec<&str> = shape_maps::CLASSES.iter().map(|(name, _)| *name).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![
            "CeilingHangingSignBlock",
            "ComposterBlock",
            "DecoratedPotBlock",
            "FlowerBedBlock",
            "HopperBlock",
            "LeafLitterBlock",
            "MossyCarpetBlock",
        ]
    );
    for (name, family) in shape_maps::CLASSES {
        assert_eq!(shape_maps::family_for_class(name), Some(*family));
    }
    // Classes owned by the other engines stay out of this module.
    assert_eq!(shape_maps::family_for_class("StairBlock"), None);
    assert_eq!(shape_maps::family_for_class("LeavesBlock"), None);
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
        let Some(family) = shape_maps::family_for_class(&row.class) else {
            continue;
        };
        let values = pairs(row);
        let state = State::new(&values);
        for (kind, table) in [
            (ShapeKind::Collision, &collision),
            (ShapeKind::Occlusion, &occlusion),
        ] {
            let Some(shape) = shape_for_class(&row.class, &state, kind) else {
                continue;
            };
            // The collision oracle leaves states it never queried at -1; the
            // occlusion table always has a shape.
            if table.cached[index] < 0 {
                uncached += 1;
                continue;
            }
            let entry = stats.entry(family_name(family)).or_default();
            entry.states += 1;
            entry.blocks.insert(row.block.clone());
            let encoded = shape.encode();
            let theirs = &table.shapes[table.cached[index] as usize];
            if let Some(diff) = same(&encoded, theirs) {
                if failures.len() < 40 {
                    failures.push(format!(
                        "state {index} {} {kind:?} {} {}: {diff}",
                        row.block,
                        family_name(family),
                        format_properties(&row.properties)
                    ));
                }
            } else {
                entry.compared += 1;
            }
        }
    }
    let mut summary = String::new();
    let mut total = 0usize;
    for (family, entry) in &stats {
        total += entry.compared;
        summary.push_str(&format!(
            "  {family}: {} blocks, {} state-shapes compared ({} covered)\n",
            entry.blocks.len(),
            entry.compared,
            entry.states
        ));
    }
    eprintln!("shape_maps coverage:\n{summary}");
    eprintln!(
        "  uncached collision states (reference writes -1): {uncached}; exact matches {total}"
    );
    assert!(
        failures.is_empty(),
        "{} covered shapes differ; first differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
    // A crash-proof lock on how much of the registry this module owns: the
    // seven classes cover 21 blocks and 1,077 states in both tables.
    assert!(
        stats.len() == 7,
        "all seven families must report coverage, got {:?}",
        stats.keys().collect::<Vec<_>>()
    );
    assert_eq!(stats["FlowerBed"].blocks.len(), 2, "pink petals + wildflowers");
    assert_eq!(stats["CeilingHangingSign"].blocks.len(), 13);
    assert_eq!(stats["MossyCarpet"].blocks.len(), 1);
    assert_eq!(stats["Hopper"].blocks.len(), 1);
    assert_eq!(stats["Composter"].blocks.len(), 1);
    assert_eq!(stats["DecoratedPot"].blocks.len(), 1);
    assert_eq!(stats["LeafLitter"].blocks.len(), 1);
}

#[test]
fn the_uncovered_static_shapes_are_reported_as_gaps() {
    if !jar().exists() || !reference(geometry::COLLISION).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let rows = state_rows();
    let collision = geometry::read_table(&reference(geometry::COLLISION)).expect("collision");
    let occlusion = geometry::read_table(&reference(geometry::OCCLUSION)).expect("occlusion");
    // A class has a non-default shape when a state's shape is neither the empty
    // nor the full-block default (canonical != 0). Classes this module owns are
    // not gaps; everything else -- the selectors of `shape_families`, the
    // offset/state-holder engines, and what none of them covers yet -- is listed
    // so the next engine step knows its work.
    let mut gaps: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        if shape_maps::family_for_class(&row.class).is_some() {
            continue;
        }
        let collision_interesting =
            collision.cached[index] >= 0 && collision.shapes[collision.cached[index] as usize].canonical == 0;
        let occlusion_interesting = occlusion.shapes[occlusion.cached[index] as usize].canonical == 0;
        if !collision_interesting && !occlusion_interesting {
            continue;
        }
        let entry = gaps.entry(row.class.clone()).or_insert((0, 0));
        entry.0 += 1;
        entry.1 = index;
    }
    let mut report = String::new();
    let mut total = 0usize;
    for (class, (count, _)) in &gaps {
        total += count;
        report.push_str(&format!("  {class}: {count} states\n"));
    }
    eprintln!(
        "classes outside this module with non-default shapes ({} states over {} classes):\n{report}",
        total,
        gaps.len()
    );
    assert!(
        !gaps.contains_key("CeilingHangingSignBlock")
            && !gaps.contains_key("ComposterBlock")
            && !gaps.contains_key("DecoratedPotBlock")
            && !gaps.contains_key("HopperBlock")
            && !gaps.contains_key("MossyCarpetBlock")
            && !gaps.contains_key("FlowerBedBlock")
            && !gaps.contains_key("LeafLitterBlock"),
        "this module's classes must not show up as gaps"
    );
}
