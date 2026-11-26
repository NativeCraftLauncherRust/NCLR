//! Does the support-shape model reproduce Horizon's prepared
//! `worldgen/blocks/cached-support.nbt`?
//!
//! Two independent checks:
//!
//! 1. **The rule itself**: feed the prepared collision table in as the
//!    collision-shape provider (that is exactly what the default
//!    `getBlockSupportShape` does) and compare the result against the prepared
//!    support table, state by state. Every reachable state must match exactly.
//! 2. **The pipeline**: run `shape_combine::combine` (families + default path +
//!    bytecode VM) for the collision shapes, then this module's support table,
//!    and report how much of the prepared support table that reaches and whether
//!    any support mismatch is *not* inherited from a collision mismatch.

#[path = "../src/block.rs"]
mod block;
#[path = "../src/shape_combine.rs"]
mod shape_combine;
#[path = "../src/shape_support.rs"]
mod shape_support;
#[path = "../src/states.rs"]
mod states;

use std::path::PathBuf;

/// The support module's copy of the table container/`Shape` type; everything in
/// this file speaks that type so the tables compare directly.
use shape_support::geometry as table;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn prepared(name: &str) -> PathBuf {
    workspace().join("tmp/ship-extract").join(name)
}

fn collision_reference() -> table::Table {
    table::read_table(&prepared(table::COLLISION)).expect("collision table")
}

fn support_reference() -> table::Table {
    table::read_table(&prepared(shape_support::SUPPORT)).expect("support table")
}

/// The prepared collision table as a `Some`/`None` collision provider (`-1` in
/// `cached_shapes` means the runtime cache had no entry).
fn prepared_collision_provider() -> Vec<Option<table::Shape>> {
    let reference = collision_reference();
    (0..reference.cached.len())
        .map(|state| {
            if reference.cached[state] < 0 {
                None
            } else {
                Some(reference.shapes[reference.cached[state] as usize].clone())
            }
        })
        .collect()
}

/// `shape_combine` carries its own path-included copy of `geometry::Shape`.
fn from_combine(shape: &shape_combine::geometry::Shape) -> table::Shape {
    table::Shape {
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

struct Comparison {
    reachable: usize,
    covered: usize,
    exact: usize,
    skipped_uncached: usize,
    missing: Vec<usize>,
    mismatches: usize,
    differences: Vec<(usize, String)>,
}

fn compare(derived: &[Option<table::Shape>], reference: &table::Table) -> Comparison {
    let mut out = Comparison {
        reachable: 0,
        covered: 0,
        exact: 0,
        skipped_uncached: 0,
        missing: Vec::new(),
        mismatches: 0,
        differences: Vec::new(),
    };
    for (state, shape) in derived.iter().enumerate() {
        if reference.cached[state] < 0 {
            out.skipped_uncached += 1;
            continue;
        }
        out.reachable += 1;
        let theirs = &reference.shapes[reference.cached[state] as usize];
        match shape {
            Some(ours) => {
                out.covered += 1;
                if ours == theirs {
                    out.exact += 1;
                } else {
                    out.mismatches += 1;
                }
                if ours != theirs && out.differences.len() < 20 {
                    out.differences.push((
                        state,
                        format!(
                            "ours canonical {} cube {} axes {} x{:?} y{:?} z{:?}; theirs canonical {} cube {} axes {} x{:?} y{:?} z{:?}",
                            ours.canonical,
                            ours.cube,
                            ours.cube_axes,
                            ours.x,
                            ours.y,
                            ours.z,
                            theirs.canonical,
                            theirs.cube,
                            theirs.cube_axes,
                            theirs.x,
                            theirs.y,
                            theirs.z
                        ),
                    ));
                }
            }
            None => out.missing.push(state),
        }
    }
    out
}

#[test]
fn the_support_rule_is_exact_against_the_prepared_table() {
    if !jar().exists() || !prepared(shape_support::SUPPORT).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let collision = prepared_collision_provider();
    let (derived, coverage) =
        shape_support::support_table(&jar(), &collision).expect("the support table");
    let reference = support_reference();
    let comparison = compare(&derived, &reference);
    eprintln!(
        "support rule: {} reachable states, {} covered, {} exact, {} uncached in the prepared copy; {} derived-but-missing",
        comparison.reachable,
        comparison.covered,
        comparison.exact,
        comparison.skipped_uncached,
        comparison.missing.len()
    );
    eprintln!(
        "  override states: {}, collision pass-through: {}, gaps: {} states over {} classes",
        coverage.overridden,
        coverage.passed_through,
        coverage.missing,
        coverage.gaps.len()
    );
    for (state, difference) in comparison.differences.iter().take(8) {
        eprintln!("  difference state {state}: {difference}");
    }
    for state in comparison.missing.iter().take(8) {
        eprintln!("  missing state {state}");
    }
    for (class, reason) in coverage.gaps.iter().take(12) {
        eprintln!("  gap {class}: {reason}");
    }
    assert_eq!(
        comparison.mismatches, 0,
        "the support rule differs on {} states",
        comparison.mismatches
    );
    assert_eq!(
        comparison.covered, comparison.reachable,
        "every state with a prepared support shape should be derived"
    );
    assert!(
        comparison.skipped_uncached > 0,
        "the prepared copy has uncached states (the -1 entries)"
    );
    assert!(
        coverage.missing <= comparison.skipped_uncached,
        "gaps ({}) cannot exceed the uncached states ({})",
        coverage.missing,
        comparison.skipped_uncached
    );
}

#[test]
fn the_combined_engine_reaches_the_support_table_except_where_collision_is_missing() {
    if !jar().exists() || !prepared(shape_support::SUPPORT).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let combined = shape_combine::combine(&jar()).expect("the combination");
    let collision: Vec<Option<table::Shape>> = combined
        .collision
        .iter()
        .map(|shape| shape.as_ref().map(from_combine))
        .collect();
    let (derived, coverage) =
        shape_support::support_table(&jar(), &collision).expect("the support table");
    let reference_support = support_reference();
    let reference_collision = collision_reference();
    let total = reference_support.cached.len();
    let comparison = compare(&derived, &reference_support);
    eprintln!(
        "support pipeline: {} of {} states covered ({:.1}%), {} exact ({:.1}%), {} missing",
        comparison.covered,
        total,
        100.0 * comparison.covered as f64 / total as f64,
        comparison.exact,
        100.0 * comparison.exact as f64 / total as f64,
        total - comparison.covered
    );
    eprintln!(
        "  override states: {}, pass-through: {}, gaps: {} states over {} classes",
        coverage.overridden,
        coverage.passed_through,
        coverage.missing,
        coverage.gaps.len()
    );
    // Any support mismatch must be inherited from a collision mismatch on the
    // same state; the support rule itself may not introduce new differences.
    // Exhaustive: every reachable state is checked, not a sample.
    let mut inherited = 0usize;
    let mut unexplained = Vec::new();
    let mut mismatches = 0usize;
    for state in 0..total {
        if reference_support.cached[state] < 0 {
            continue;
        }
        let Some(ours) = &derived[state] else {
            continue;
        };
        let theirs = &reference_support.shapes[reference_support.cached[state] as usize];
        if ours == theirs {
            continue;
        }
        mismatches += 1;
        let collision_ours = collision[state].as_ref();
        let collision_theirs = if reference_collision.cached[state] >= 0 {
            Some(&reference_collision.shapes[reference_collision.cached[state] as usize])
        } else {
            None
        };
        if collision_ours == collision_theirs {
            unexplained.push(state);
        } else {
            inherited += 1;
        }
    }
    eprintln!(
        "  support mismatches: {mismatches} total, {inherited} inherited from collision, {} unexplained",
        unexplained.len()
    );
    for state in unexplained.iter().take(8) {
        eprintln!("  unexplained support difference at state {state}");
    }
    assert!(
        unexplained.is_empty(),
        "support introduced new differences at states {unexplained:?}"
    );
    assert_eq!(mismatches, inherited, "all mismatches are inherited");
    assert!(
        comparison.covered as f64 / total as f64 > 0.95,
        "the pipeline should reach well over the previous 61.1%"
    );
}

#[test]
fn every_jar_class_with_the_override_is_modelled() {
    if !jar().exists() {
        eprintln!("skipped: needs the jar");
        return;
    }
    let registry = block::registry(&jar()).expect("registry");
    let bases = [
        "LeavesBlock",
        "MudBlock",
        "SoulSandBlock",
        "ShulkerBoxBlock",
        "SnowLayerBlock",
        "ChorusFlowerBlock",
        "FenceGateBlock",
        "CeilingHangingSignBlock",
        "WallHangingSignBlock",
    ];
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut unmodelled = Vec::new();
    for row in &registry {
        let overrides = row
            .chain
            .iter()
            .any(|class| bases.contains(&class.as_str()));
        if !overrides {
            continue;
        }
        seen.insert(row.class.clone());
        if !shape_support::class_overrides_support(&row.class) {
            unmodelled.push(format!("{} ({})", row.id, row.class));
        }
    }
    eprintln!("classes inheriting a support override: {seen:?}");
    assert!(
        unmodelled.is_empty(),
        "classes that inherit getBlockSupportShape but are not modelled: {unmodelled:?}"
    );
    assert!(
        seen.len() >= 10,
        "the jar should exercise the override classes"
    );
}

#[test]
fn the_gap_states_are_the_uncached_ones() {
    if !jar().exists() || !prepared(shape_support::SUPPORT).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let collision = prepared_collision_provider();
    let (derived, coverage) =
        shape_support::support_table(&jar(), &collision).expect("the support table");
    let reference = support_reference();
    assert_eq!(
        derived.len(),
        reference.cached.len(),
        "one shape slot per state"
    );
    let total = reference.cached.len();
    let mut uncached_missing = 0usize;
    let mut uncached_derived = 0usize;
    for state in 0..total {
        if reference.cached[state] >= 0 {
            continue;
        }
        if derived[state].is_some() {
            uncached_derived += 1;
        } else {
            uncached_missing += 1;
        }
    }
    eprintln!(
        "uncached states: {uncached_derived} derived by an override, {uncached_missing} reported as gaps"
    );
    assert_eq!(
        uncached_missing, coverage.missing,
        "the only missing states are the uncached non-override ones"
    );
    eprintln!(
        "gap classes: {:?}",
        coverage.gaps.keys().collect::<Vec<_>>()
    );
}

