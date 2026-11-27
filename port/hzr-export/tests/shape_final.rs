//! End-to-end validation of the final shape-table integration.
//!
//! `src/shape_final.rs` combines every engine (`shape_gate`, `shape_stateholder`,
//! `shape_maps`, `shape_families`, the default path, the offset-aware step-2b
//! VM and `shape_support`) into one evaluator. This test drives it over all
//! 35,723 registry states, compares every entry of `cached-collision.nbt`,
//! `occlusion.nbt`, `worldgen/blocks/cached-support.nbt` and
//! `structure-blocks.json#full_block` with the prepared copies in
//! `tmp/ship-extract`, and checks the written bundle decodes back to the same
//! tables.
//!
//! Per state the check is:
//!
//! * the *uncached* pattern (`cached_shapes == -1`, the `dynamicShape` blocks)
//!   must match exactly;
//! * every other entry must be **byte-identical** (the same dictionary entry:
//!   coordinates, occupancy, discrete bounds and the `canonical`/`cube`/
//!   `cube_axes` class flags) or, where an engine reproduces the point set with
//!   a different coordinate partition, **point-set equal**;
//! * anything an engine cannot derive stays `None` and is reported per class —
//!   nothing is guessed.
//!
//! The test also writes the bundle to a temporary directory and re-reads it with
//! `geometry::read_table`, so the encoder path is covered as well.

#[path = "../src/shape_final.rs"]
mod shape_final;

// The evaluator's own copies of the state/registry/table modules: the table
// types must be the same instances the evaluator produces.
use shape_final::{block, geometry, states};

use std::collections::BTreeMap;
use std::path::PathBuf;

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
// Point-set equality on the table encoding
// ---------------------------------------------------------------------------

fn axis(shape: &geometry::Shape, index: usize) -> &Vec<f64> {
    match index {
        0 => &shape.x,
        1 => &shape.y,
        _ => &shape.z,
    }
}

fn sizes(shape: &geometry::Shape) -> [usize; 3] {
    [
        shape.x.len().saturating_sub(1),
        shape.y.len().saturating_sub(1),
        shape.z.len().saturating_sub(1),
    ]
}

fn contains(shape: &geometry::Shape, point: [f64; 3]) -> bool {
    let sizes = sizes(shape);
    let mut cell = [0usize; 3];
    for index in 0..3 {
        let coords = axis(shape, index);
        if point[index] < coords[0] || point[index] > coords[coords.len() - 1] {
            return false;
        }
        let after = coords.partition_point(|value| *value <= point[index]);
        if after == 0 || after > coords.len() - 1 {
            return false;
        }
        cell[index] = after - 1;
    }
    if cell[0] >= sizes[0] || cell[1] >= sizes[1] || cell[2] >= sizes[2] {
        return false;
    }
    shape.occupied[(cell[0] * sizes[1] + cell[1]) * sizes[2] + cell[2]] != 0
}

/// Point-set equality: both shapes occupy the same part of the block, whatever
/// coordinate partition each one uses.
fn equivalent(actual: &geometry::Shape, expected: &geometry::Shape) -> bool {
    if actual == expected {
        return true;
    }
    if actual.canonical == 1 || expected.canonical == 1 {
        return actual.canonical == expected.canonical;
    }
    let mut coords: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for index in 0..3 {
        let mut values: Vec<f64> = axis(expected, index)
            .iter()
            .chain(axis(actual, index).iter())
            .copied()
            .collect();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        values.dedup_by(|a, b| a.to_bits() == b.to_bits());
        coords[index] = values;
    }
    let sizes = [
        coords[0].len().saturating_sub(1),
        coords[1].len().saturating_sub(1),
        coords[2].len().saturating_sub(1),
    ];
    for x in 0..sizes[0] {
        for y in 0..sizes[1] {
            for z in 0..sizes[2] {
                let point = [
                    (coords[0][x] + coords[0][x + 1]) / 2.0,
                    (coords[1][y] + coords[1][y + 1]) / 2.0,
                    (coords[2][z] + coords[2][z + 1]) / 2.0,
                ];
                if contains(actual, point) != contains(expected, point) {
                    return false;
                }
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Comparison helper
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Report {
    exact: usize,
    repartitioned: usize,
    /// We produced a shape where the oracle stores `-1` (must stay zero).
    unexpected: usize,
    /// The oracle has a shape and we have none.
    gaps: usize,
    /// Repartitioned entries per class, for the report.
    repartitioned_classes: BTreeMap<String, usize>,
    /// Both are cached but the point sets differ (must stay zero).
    wrong: Vec<String>,
    /// The uncached `-1` patterns differ (must stay zero).
    uncached_mismatch: Vec<String>,
    gap_classes: BTreeMap<String, usize>,
}

fn compare(
    label: &str,
    ours: &[Option<geometry::Shape>],
    reference: &geometry::Table,
    classes: &[String],
) -> Report {
    let mut report = Report::default();
    assert_eq!(ours.len(), reference.cached.len(), "{label}: one entry per state");
    for (state, shape) in ours.iter().enumerate() {
        let cached = reference.cached[state];
        match (shape, cached < 0) {
            (None, true) => {}
            (None, false) => {
                report.gaps += 1;
                *report
                    .gap_classes
                    .entry(classes[state].clone())
                    .or_insert(0) += 1;
            }
            (Some(_), true) => {
                report.unexpected += 1;
                if report.uncached_mismatch.len() < 10 {
                    report.uncached_mismatch.push(format!(
                        "{label} state {state}: we derive a shape, the oracle stores -1"
                    ));
                }
            }
            (Some(our_shape), false) => {
                let theirs = &reference.shapes[cached as usize];
                if our_shape == theirs {
                    report.exact += 1;
                } else if equivalent(our_shape, theirs) {
                    report.repartitioned += 1;
                    *report
                        .repartitioned_classes
                        .entry(classes[state].clone())
                        .or_insert(0) += 1;
                } else if report.wrong.len() < 10 {
                    report.wrong.push(format!(
                        "{label} state {state}: ours canonical {} cube {} axes {} occ {}; theirs canonical {} cube {} axes {} occ {}",
                        our_shape.canonical,
                        our_shape.cube,
                        our_shape.cube_axes,
                        our_shape.occupied.len(),
                        theirs.canonical,
                        theirs.cube,
                        theirs.cube_axes,
                        theirs.occupied.len()
                    ));
                }
            }
        }
    }
    // Every `-1` the oracle stores must be `-1` in ours too.
    for (state, cached) in reference.cached.iter().enumerate() {
        if *cached < 0 && ours[state].is_some() {
            report.unexpected += 1;
        }
    }
    report
}

/// The block class of every state, in registry order (for the gap report).
fn per_state_classes(jar_path: &std::path::Path) -> Vec<String> {
    let registry = block::registry(jar_path).expect("the block registry");
    let table = states::state_table(jar_path).expect("the state table");
    assert_eq!(registry.len(), table.blocks.len(), "registry order");
    let mut out = Vec::new();
    for (block_index, row) in table.blocks.iter().enumerate() {
        let class = registry[block_index].class.clone();
        for _ in 0..row.state_count {
            out.push(class.clone());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn the_tables_cover_and_match_the_prepared_copies() {
    if !jar().exists() || !prepared(shape_final::COLLISION).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let evaluated = shape_final::evaluate(&jar()).expect("the evaluation");
    let classes = per_state_classes(&jar());
    let collision = geometry::read_table(&prepared(shape_final::COLLISION)).expect("collision");
    let occlusion = geometry::read_table(&prepared(shape_final::OCCLUSION)).expect("occlusion");
    let support = geometry::read_table(&prepared(shape_final::SUPPORT)).expect("support");
    assert_eq!(evaluated.collision.len(), collision.cached.len(), "state count");
    assert_eq!(evaluated.occlusion.len(), occlusion.cached.len(), "state count");
    assert_eq!(evaluated.support.len(), support.cached.len(), "state count");

    let mut failures: Vec<String> = Vec::new();
    let mut total_exact = 0usize;
    let mut total_repartitioned = 0usize;
    let mut total_gaps = 0usize;
    for (label, ours, reference) in [
        ("collision", &evaluated.collision, &collision),
        ("occlusion", &evaluated.occlusion, &occlusion),
        ("support", &evaluated.support, &support),
    ] {
        let report = compare(label, ours, reference, &classes);
        eprintln!(
            "{label}: ours some {} none {}, oracle -1 {}, oracle cached {}",
            ours.iter().filter(|shape| shape.is_some()).count(),
            ours.iter().filter(|shape| shape.is_none()).count(),
            reference.cached.iter().filter(|value| **value < 0).count(),
            reference.cached.iter().filter(|value| **value >= 0).count(),
        );
        eprintln!(
            "{label}: exact {exact}, repartitioned {repartitioned}, gaps {gaps}, unexpected {unexpected} of {total} states",
            exact = report.exact,
            repartitioned = report.repartitioned,
            gaps = report.gaps,
            unexpected = report.unexpected,
            total = ours.len(),
        );
        let mut repartitioned: Vec<(String, usize)> = report
            .repartitioned_classes
            .iter()
            .map(|(class, count)| (class.clone(), *count))
            .collect();
        repartitioned.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        if !repartitioned.is_empty() {
            eprintln!("{label} repartitioned classes: {repartitioned:?}");
        }
        let mut gap_classes: Vec<(String, usize)> = report.gap_classes.iter().map(|(k, v)| (k.clone(), *v)).collect();
        gap_classes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let mut gap_report = String::new();
        for (class, count) in gap_classes.iter().take(40) {
            gap_report.push_str(&format!("\n  {class}: {count} states"));
        }
        eprintln!("{label} gap classes:{}", gap_report);
        failures.extend(report.wrong);
        failures.extend(report.uncached_mismatch);
        total_exact += report.exact;
        total_repartitioned += report.repartitioned;
        total_gaps += report.gaps;
        assert_eq!(
            report.unexpected, 0,
            "{label}: we must not invent a shape where the oracle stores -1"
        );
    }

    // `full_block` is `!hasDynamicShape && isCollisionShapeFullBlock`.
    let text = std::fs::read_to_string(prepared("worldgen/blocks/structure-blocks.json"))
        .expect("structure-blocks.json");
    let document: serde_json::Value = serde_json::from_str(&text).expect("JSON");
    let reference = document["full_block"].as_array().expect("full_block array");
    assert_eq!(reference.len(), evaluated.full_block.len(), "full_block length");
    let mut full_exact = 0usize;
    let mut full_gaps = 0usize;
    for (state, flag) in evaluated.full_block.iter().enumerate() {
        let theirs = reference[state].as_bool().unwrap_or(false);
        match flag {
            Some(value) if *value == theirs => full_exact += 1,
            Some(_) => failures.push(format!(
                "full_block state {state}: ours differs from the reference"
            )),
            None => full_gaps += 1,
        }
    }
    eprintln!(
        "full_block: exact {full_exact}, gaps {full_gaps} of {} states",
        evaluated.full_block.len()
    );
    eprintln!(
        "totals: {} exact, {} point-set equal with a different partition, {} gaps",
        total_exact, total_repartitioned, total_gaps
    );
    eprintln!(
        "dynamic-shape states (uncached -1): {}",
        evaluated.coverage.dynamic_states
    );
    let mut gaps: Vec<(String, String)> = evaluated
        .coverage
        .gaps
        .iter()
        .map(|(key, reason)| (key.clone(), reason.clone()))
        .collect();
    gaps.sort();
    for (key, reason) in gaps.iter().take(40) {
        eprintln!("gap {key}: {reason}");
    }

    assert!(
        failures.is_empty(),
        "{} entry problems; first ones:\n{}",
        failures.len(),
        failures.join("\n")
    );
    // Lock the coverage this step reaches. The remaining gaps are the classes
    // the VM cannot evaluate yet; the test reports them by name above.
    assert!(
        evaluated.coverage.collision_covered >= 35_300,
        "collision covered {}",
        evaluated.coverage.collision_covered
    );
    assert!(
        evaluated.coverage.occlusion_covered >= 35_000,
        "occlusion covered {}",
        evaluated.coverage.occlusion_covered
    );
    assert!(
        evaluated.coverage.support_covered >= 35_100,
        "support covered {}",
        evaluated.coverage.support_covered
    );
    assert_eq!(evaluated.coverage.dynamic_states, 199, "dynamic-shape states");
    assert_eq!(full_exact + full_gaps, reference.len(), "full_block accounted");
}

#[test]
fn the_written_bundle_decodes_back_to_the_same_tables() {
    if !jar().exists() || !prepared(shape_final::COLLISION).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let evaluated = shape_final::evaluate(&jar()).expect("the evaluation");
    let directory = std::env::temp_dir().join(format!(
        "hzr-export-shape-final-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("the output directory");
    shape_final::write_bundle(&evaluated, &directory).expect("the written bundle");

    let collision = geometry::read_table(&directory.join(shape_final::COLLISION)).expect("collision");
    let occlusion = geometry::read_table(&directory.join(shape_final::OCCLUSION)).expect("occlusion");
    let support = geometry::read_table(&directory.join(shape_final::SUPPORT)).expect("support");
    let reference = geometry::read_table(&prepared(shape_final::SUPPORT)).expect("support oracle");

    for (label, table, shapes, kind) in [
        ("collision", &collision, &evaluated.collision, None),
        ("occlusion", &occlusion, &evaluated.occlusion, None),
        ("support", &support, &evaluated.support, Some(shape_final::SUPPORT_KIND)),
    ] {
        assert_eq!(table.kind.as_deref(), kind, "{label}: Kind");
        assert_eq!(table.data_version, shape_final::DATA_VERSION, "{label}: DataVersion");
        assert_eq!(table.cached.len(), shapes.len(), "{label}: one entry per state");
        for (state, shape) in shapes.iter().enumerate() {
            match (shape, table.cached[state]) {
                (None, cached) => assert_eq!(cached, -1, "{label} state {state}: expected -1"),
                (Some(shape), cached) => {
                    assert!(cached >= 0, "{label} state {state}: a shape must be cached");
                    assert_eq!(
                        shape,
                        &table.shapes[cached as usize],
                        "{label} state {state}: decoded shape"
                    );
                }
            }
        }
    }
    // The written support table keeps the oracle's `-1` pattern.
    let written_uncached: usize = support.cached.iter().filter(|value| **value < 0).count();
    let oracle_uncached: usize = reference.cached.iter().filter(|value| **value < 0).count();
    assert_eq!(written_uncached, oracle_uncached, "support uncached pattern");

    let text = std::fs::read_to_string(directory.join(shape_final::FULL_BLOCK))
        .expect("the full_block fragment");
    let document: serde_json::Value = serde_json::from_str(&text).expect("JSON");
    let written = document["full_block"].as_array().expect("full_block array");
    assert_eq!(written.len(), evaluated.full_block.len(), "full_block length");
    for (state, flag) in evaluated.full_block.iter().enumerate() {
        match flag {
            Some(value) => assert_eq!(
                written[state].as_bool(),
                Some(*value),
                "full_block state {state}"
            ),
            None => assert!(written[state].is_null(), "full_block state {state} null"),
        }
    }
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn the_dynamic_shape_blocks_are_exactly_the_uncached_ones() {
    if !jar().exists() {
        eprintln!("skipped: needs the jar");
        return;
    }
    let profile = shape_final::class_profiles(&jar()).expect("the class profiles");
    eprintln!(
        "dynamic-shape classes: {:?}\nno-collision classes: {} colliding classes: {}",
        profile.dynamic,
        profile.no_collision.len(),
        profile.colliding.len()
    );
    assert_eq!(profile.dynamic.len(), 7, "7 dynamicShape classes");
    assert!(profile.no_collision.contains("BannerBlock"));
    assert!(profile.colliding.contains("StairBlock"));
}
