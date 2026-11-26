//! Do the fence-gate and cave-vine selectors reproduce Horizon's prepared tables?
//!
//! The reference is `tmp/ship-extract`: `cached-collision.nbt` and
//! `occlusion.nbt`, indexed by state id, plus `_shared/states.bin` for the block
//! id of every state. The state -> property map is rebuilt here from the jar
//! (`states::state_table`), so the selector is checked against every state of
//! every covered block, not against hand-picked examples.
//!
//! Every covered state must match exactly: coordinates, occupancy, discrete
//! bounds and the `canonical`/`cube`/`cube_axes` class flags. The support table
//! is checked the same way when the prepared copy is present.

#[path = "../src/block.rs"]
mod block;
#[path = "../src/geometry.rs"]
mod geometry;
#[path = "../src/shape_gate.rs"]
mod shape_gate;
#[path = "../src/states.rs"]
mod states;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use shape_gate::{shape_for_class, EncodedShape, Method, State};

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
    /// The superclass chain from the block's class up to `Block`.
    chain: Vec<String>,
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
        let combos = cartesian(
            &sorted
                .iter()
                .map(|property| (*property).clone())
                .collect::<Vec<_>>(),
        );
        assert_eq!(combos.len(), block.state_count, "{} state count", block.id);
        for combo in combos {
            rows.push(Row {
                block: block.id.clone(),
                class: registered.class.clone(),
                chain: registered.chain.clone(),
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

fn same(encoded: &EncodedShape, theirs: &geometry::Shape) -> Option<String> {
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

/// Per-class coverage of one table pair: states with a reference entry and
/// state-shapes that matched exactly.
#[derive(Default)]
struct Stats {
    blocks: BTreeSet<String>,
    states: usize,
    collision_checked: usize,
    collision_exact: usize,
    occlusion_checked: usize,
    occlusion_exact: usize,
}

fn format_properties(properties: &[(String, String)]) -> String {
    properties
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join(",")
}

#[test]
fn every_covered_state_matches_the_prepared_collision_and_occlusion_tables() {
    if !jar().exists() || !reference(geometry::COLLISION).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let rows = state_rows();
    let collision = geometry::read_table(&reference(geometry::COLLISION)).expect("collision");
    let occlusion = geometry::read_table(&reference(geometry::OCCLUSION)).expect("occlusion");
    assert_eq!(rows.len(), collision.cached.len(), "state count");
    assert_eq!(rows.len(), occlusion.cached.len(), "state count");
    let prepared = geometry::state_blocks(&workspace().join("tmp/ship-extract/_shared/states.bin"))
        .expect("the prepared states");
    assert_eq!(prepared.len(), rows.len(), "prepared states");
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(
            prepared[index].0, row.block,
            "state {index} belongs to another block"
        );
    }
    let mut stats: BTreeMap<String, Stats> = BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();
    let mut uncached = 0usize;
    for (index, row) in rows.iter().enumerate() {
        if shape_gate::family_for_class(&row.class).is_none() {
            continue;
        }
        let values = pairs(row);
        let state = State::new(&values);
        let entry = stats.entry(row.class.clone()).or_default();
        entry.blocks.insert(row.block.clone());
        entry.states += 1;
        for (method, table) in [
            (Method::Collision, &collision),
            (Method::Occlusion, &occlusion),
        ] {
            if table.cached[index] < 0 {
                uncached += 1;
                continue;
            }
            let shape = shape_for_class(&row.class, &state, method).expect("a covered class");
            let encoded = shape.encode();
            let theirs = &table.shapes[table.cached[index] as usize];
            let checked = match method {
                Method::Collision => &mut entry.collision_checked,
                Method::Occlusion => &mut entry.occlusion_checked,
                _ => unreachable!(),
            };
            *checked += 1;
            match same(&encoded, theirs) {
                Some(diff) => {
                    if failures.len() < 40 {
                        failures.push(format!(
                            "state {index} {} {} ({}): {diff}",
                            row.block,
                            method.name(),
                            format_properties(&row.properties)
                        ));
                    }
                }
                None => match method {
                    Method::Collision => entry.collision_exact += 1,
                    Method::Occlusion => entry.occlusion_exact += 1,
                    _ => unreachable!(),
                },
            }
        }
    }
    let mut states = 0usize;
    let mut report = String::new();
    for (class, entry) in &stats {
        states += entry.states;
        report.push_str(&format!(
            "  {class}: {} blocks, {} states, getCollisionShape {}/{} exact, getOcclusionShape {}/{} exact\n",
            entry.blocks.len(),
            entry.states,
            entry.collision_exact,
            entry.collision_checked,
            entry.occlusion_exact,
            entry.occlusion_checked,
        ));
    }
    let expected_shapes: usize = stats.values().map(|entry| entry.states * 2).sum();
    let checked: usize = stats
        .values()
        .map(|entry| entry.collision_checked + entry.occlusion_checked)
        .sum();
    eprintln!(
        "shape_gate coverage ({states} states over {} blocks):\n{report}",
        stats
            .values()
            .map(|entry| entry.blocks.len())
            .sum::<usize>()
    );
    eprintln!(
        "  {checked}/{expected_shapes} state-shapes checked, {uncached} uncached reference entries"
    );
    assert!(
        failures.is_empty(),
        "{} covered shapes differ; first differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert_eq!(
        checked, expected_shapes,
        "every covered state needs both reference shapes"
    );
    assert_eq!(uncached, 0, "the prepared copies cache every covered state");
}

#[test]
fn the_support_shapes_match_the_prepared_support_table() {
    if !jar().exists() || !reference(geometry::SUPPORT).exists() {
        eprintln!("skipped: needs the jar and the prepared support table");
        return;
    }
    let rows = state_rows();
    let support = geometry::read_table(&reference(geometry::SUPPORT)).expect("support");
    assert_eq!(rows.len(), support.cached.len(), "state count");
    let mut checked = 0usize;
    let mut exact = 0usize;
    let mut uncached = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        if shape_gate::family_for_class(&row.class).is_none() {
            continue;
        }
        if support.cached[index] < 0 {
            uncached += 1;
            continue;
        }
        let values = pairs(row);
        let state = State::new(&values);
        let encoded = shape_for_class(&row.class, &state, Method::Support)
            .expect("a covered class")
            .encode();
        let theirs = &support.shapes[support.cached[index] as usize];
        checked += 1;
        match same(&encoded, theirs) {
            Some(diff) => {
                if failures.len() < 40 {
                    failures.push(format!(
                        "state {index} {} getBlockSupportShape ({}): {diff}",
                        row.block,
                        format_properties(&row.properties)
                    ));
                }
            }
            None => exact += 1,
        }
    }
    eprintln!("shape_gate support coverage: {exact}/{checked} exact, {uncached} uncached");
    assert!(
        failures.is_empty(),
        "{} covered support shapes differ; first differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert_eq!(exact, checked, "every covered support shape must match");
    assert_eq!(uncached, 0, "the prepared copy caches every covered state");
}

#[test]
fn the_coverage_report_names_the_remaining_growing_plant_gaps() {
    if !jar().exists() || !reference(geometry::COLLISION).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let rows = state_rows();
    let mut covered_states = 0usize;
    let mut missing_fence_gate = Vec::new();
    let mut gaps: BTreeMap<String, (usize, BTreeSet<String>)> = BTreeMap::new();
    for row in &rows {
        if shape_gate::family_for_class(&row.class).is_some() {
            covered_states += 1;
            continue;
        }
        if row.chain.iter().any(|class| class == "FenceGateBlock") {
            missing_fence_gate.push(row.block.clone());
            continue;
        }
        if row.chain.iter().any(|class| class == "GrowingPlantBlock") {
            let entry = gaps
                .entry(row.class.clone())
                .or_insert_with(|| (0, BTreeSet::new()));
            entry.0 += 1;
            entry.1.insert(row.block.clone());
        }
    }
    // 13 fence gates with 32 states each + 52 cave-vine heads + 2 bodies.
    assert_eq!(covered_states, 470, "every fence gate and cave vine state");
    assert!(
        missing_fence_gate.is_empty(),
        "fence gates left on the default path: {missing_fence_gate:?}"
    );
    let mut report = String::new();
    let mut gap_states = 0usize;
    for (class, (states, blocks)) in &gaps {
        gap_states += states;
        report.push_str(&format!(
            "  {class}: {states} states ({})\n",
            blocks.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    eprintln!(
        "shape_gate gaps: the other GrowingPlantBlock families ({gap_states} states over {} classes):\n{report}",
        gaps.len()
    );
    let expected: BTreeSet<&str> = [
        "KelpBlock",
        "KelpPlantBlock",
        "TwistingVinesBlock",
        "TwistingVinesPlantBlock",
        "WeepingVinesBlock",
        "WeepingVinesPlantBlock",
    ]
    .into_iter()
    .collect();
    let found: BTreeSet<&str> = gaps.keys().map(|class| class.as_str()).collect();
    assert_eq!(
        found, expected,
        "the gap list names the remaining GrowingPlantBlock families"
    );
    assert_eq!(gap_states, 81, "the related families' state count");
}
