//! End-to-end check that `shape_scaffold::refine` closes the last gaps.
//!
//! Runs the integrated evaluator (`shape_final::evaluate`), applies the
//! scaffolding/straw-bed refinement, and compares every entry of the three
//! prepared tables (`cached-collision.nbt`, `occlusion.nbt`,
//! `worldgen/blocks/cached-support.nbt`) with `tmp/ship-extract`: the refined
//! bundle is byte-identical for every cached state, and the uncached
//! (`dynamicShape`) entries stay `-1`.
//!
//! Before this module the integrated bundle had 32 `ScaffoldingBlock`
//! occlusion gaps and 4 `StrawBedBlock` point-set-only entries in each of the
//! three tables. After it:
//!
//! ```text
//! cached-collision: 35,524 exact + 199 uncached
//! occlusion:        35,723 exact
//! cached-support:   35,524 exact + 199 uncached
//! ```

#[path = "../src/shape_scaffold.rs"]
mod shape_scaffold;

use shape_scaffold::final_shapes::{self, block, geometry, states};

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

fn per_state_classes(jar_path: &std::path::Path) -> Vec<String> {
    let registry = block::registry(jar_path).expect("registry");
    let table = states::state_table(jar_path).expect("state table");
    let mut out = Vec::new();
    for (block_index, row) in table.blocks.iter().enumerate() {
        for _ in 0..row.state_count {
            out.push(registry[block_index].class.clone());
        }
    }
    out
}

/// Compare one table; returns `(exact, uncached, failures)`.
fn compare(
    label: &str,
    ours: &[Option<geometry::Shape>],
    reference: &geometry::Table,
    classes: &[String],
) -> (usize, usize, Vec<String>) {
    assert_eq!(ours.len(), reference.cached.len(), "{label}: state count");
    let mut exact = 0usize;
    let mut uncached = 0usize;
    let mut failures = Vec::new();
    for (index, slot) in reference.cached.iter().enumerate() {
        if *slot < 0 {
            if ours[index].is_some() {
                failures.push(format!(
                    "{label} state {index} {}: expected uncached, got a shape",
                    classes[index]
                ));
            } else {
                uncached += 1;
            }
            continue;
        }
        let expected = &reference.shapes[*slot as usize];
        match &ours[index] {
            None => failures.push(format!(
                "{label} state {index} {}: expected a shape, got None",
                classes[index]
            )),
            Some(actual) if actual == expected => exact += 1,
            Some(actual) => failures.push(format!(
                "{label} state {index} {}: {}",
                classes[index],
                difference(expected, actual)
            )),
        }
    }
    (exact, uncached, failures)
}

/// The first differing dictionary field, for a readable failure message.
fn difference(expected: &geometry::Shape, actual: &geometry::Shape) -> String {
    if expected.x != actual.x {
        format!("x expected {:?} actual {:?}", expected.x, actual.x)
    } else if expected.y != actual.y {
        format!("y expected {:?} actual {:?}", expected.y, actual.y)
    } else if expected.z != actual.z {
        format!("z expected {:?} actual {:?}", expected.z, actual.z)
    } else if (expected.canonical, expected.cube, expected.cube_axes)
        != (actual.canonical, actual.cube, actual.cube_axes)
    {
        format!(
            "flags expected (can={}, cube={}, axes={}) actual (can={}, cube={}, axes={})",
            expected.canonical,
            expected.cube,
            expected.cube_axes,
            actual.canonical,
            actual.cube,
            actual.cube_axes
        )
    } else if (expected.discrete_min, expected.discrete_max)
        != (actual.discrete_min, actual.discrete_max)
    {
        format!(
            "discrete expected (min={:?}, max={:?}) actual (min={:?}, max={:?})",
            expected.discrete_min, expected.discrete_max, actual.discrete_min, actual.discrete_max
        )
    } else {
        "occupancy differs".to_string()
    }
}

#[test]
fn the_refined_bundle_matches_every_prepared_entry() {
    if !jar().exists() || !prepared(shape_scaffold::final_shapes::OCCLUSION).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let mut evaluated = final_shapes::evaluate(&jar()).expect("evaluation");
    shape_scaffold::refine(&jar(), &mut evaluated).expect("refinement");
    let classes = per_state_classes(&jar());

    for (label, ours, name) in [
        ("collision", &evaluated.collision, shape_scaffold::final_shapes::COLLISION),
        ("occlusion", &evaluated.occlusion, shape_scaffold::final_shapes::OCCLUSION),
        ("support", &evaluated.support, shape_scaffold::final_shapes::SUPPORT),
    ] {
        let table = geometry::read_table(&prepared(name)).expect("prepared table");
        let (exact, uncached, failures) = compare(label, ours, &table, &classes);
        eprintln!("{label}: exact {exact}, uncached {uncached}, failures {}", failures.len());
        for failure in failures.iter().take(6) {
            for chunk in failure.as_bytes().chunks(100) {
                eprintln!("  {}", String::from_utf8_lossy(chunk));
            }
        }
        assert!(
            failures.is_empty(),
            "{label}: {} entries still disagree",
            failures.len()
        );
        assert_eq!(
            exact + uncached,
            table.cached.len(),
            "{label}: every entry accounted for"
        );
    }

    // The refinement also updates the coverage counters it fills.
    assert_eq!(evaluated.coverage.occlusion_covered, 35_723);
    assert!(
        !evaluated
            .coverage
            .gaps
            .contains_key("occlusion net/minecraft/world/level/block/ScaffoldingBlock")
    );
}

#[test]
fn scaffolding_shapes_match_the_class_construction() {
    let (stable, unstable) = shape_scaffold::scaffolding_shapes_for_test();
    // SHAPE_STABLE = 16x2 top slab plus four 2x2 full-height corner posts.
    assert_eq!(stable.canonical, 0);
    assert_eq!(stable.cube_axes, 7);
    assert_eq!(stable.x.len(), 9);
    assert_eq!(stable.y.len(), 9);
    assert_eq!(stable.z.len(), 9);
    let stable_volume: usize = stable.occupied.iter().map(|cell| *cell as usize).sum();
    assert_eq!(stable_volume, 92, "top slab + four posts, shared cells removed");
    // SHAPE_UNSTABLE adds the 16x2 bottom slab (`Block.column(16, 0, 2)`).
    assert_eq!(unstable.cube_axes, 3);
    let unstable_volume: usize = unstable.occupied.iter().map(|cell| *cell as usize).sum();
    assert_eq!(unstable_volume, 152);
}

#[test]
fn straw_bed_head_rotation_keeps_the_cube_axis_bits() {
    let (foot, head) = shape_scaffold::straw_bed_shapes_for_test();
    // `Block.column(16, 0, 4)` is a CubeVoxelShape.
    assert_eq!((foot.cube, foot.cube_axes), (1, 7));
    assert_eq!(foot.x, vec![0.0, 1.0]);
    assert_eq!(foot.z, vec![0.0, 1.0]);
    // `Shapes.or(BASE, PILLOW)` keeps PILLOW's larger y list; x stays a
    // CubePointRange (the `IdenticalMerger` keeps the left operand's list).
    assert_eq!(head.cube_axes, 1);
    let rotations = shape_scaffold::straw_bed_head_rotations_for_test();
    assert_eq!(rotations[0].cube_axes, 1, "north = identity");
    assert_eq!(rotations[1].cube_axes, 4, "east = Y90");
    assert_eq!(rotations[2].cube_axes, 0, "south = Y180");
    assert_eq!(rotations[3].cube_axes, 0, "west = Y270");
    for rotation in &rotations {
        assert_eq!(rotation.cube, 0);
        assert_eq!(rotation.canonical, 0);
    }
}
