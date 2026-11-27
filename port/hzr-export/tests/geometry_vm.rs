//! Invariants the shape dictionaries obey, and the VM's target list.
//!
//! These tests pin down the encoding the VoxelShape VM has to reproduce; they
//! use the container codec from `geometry.rs` (byte-identical round-trip is
//! covered there).

#[path = "../src/geometry.rs"]
mod geometry;

#[path = "../src/geometry_vm.rs"]
mod geometry_vm;

use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn reference(name: &str) -> PathBuf {
    workspace().join("tmp/ship-extract").join(name)
}

#[test]
fn the_dictionary_invariants_hold() {
    if !reference(geometry::COLLISION).exists() {
        eprintln!("skipped: needs the prepared copies");
        return;
    }
    for name in geometry::TABLES {
        let table = geometry::read_table(&reference(name)).expect("the prepared table");
        assert!(!table.shapes.is_empty());
        for (index, shape) in table.shapes.iter().enumerate() {
            // Occupancy is exactly the cell grid of the coordinate partition.
            let expected = geometry_vm::occupancy_cells(
                shape.x.len(),
                shape.y.len(),
                shape.z.len(),
                shape.canonical,
            );
            assert_eq!(
                shape.occupied.len(),
                expected,
                "{name} shape {index}: occupancy cells"
            );
            // `firstFull`/`lastFull` stay inside the grid, with `lastFull` one
            // past the last occupied cell.
            for axis in 0..3 {
                let cells = [shape.x.len(), shape.y.len(), shape.z.len()][axis].saturating_sub(1);
                assert!(shape.discrete_min[axis] >= 0);
                assert!(shape.discrete_max[axis] >= shape.discrete_min[axis]);
                assert!(
                    shape.discrete_max[axis] as usize <= cells.max(1),
                    "{name} shape {index} axis {axis}: lastFull past the grid"
                );
            }
            // `Shapes.empty()` has no boxes and no occupancy.
            if shape.canonical == 1 {
                assert_eq!(shape.occupied.len(), 0, "{name} shape {index}: empty");
            }
            // `Shapes.block()` is the 0..1 cube with a single occupied cell.
            if shape.canonical == 2 {
                assert_eq!((shape.x.len(), shape.y.len(), shape.z.len()), (2, 2, 2));
                assert_eq!(shape.occupied, vec![1], "{name} shape {index}: block");
                assert_eq!(shape.cube, 1);
                assert_eq!(shape.cube_axes, 7);
            }
        }
        // One index per state; -1 marks a state whose shape is not cached
        // (dynamic shapes), everything else is inside the dictionary.
        assert_eq!(table.cached.len(), 35_723, "{name}: one index per state");
        let max = table.shapes.len() as i32 - 1;
        for (state, slot) in table.cached.iter().enumerate() {
            assert!(
                *slot == -1 || (0..=max).contains(slot),
                "{name} state {state}: index {slot} outside the dictionary"
            );
        }
    }
}

#[test]
fn the_vm_targets_are_the_three_shape_tables() {
    assert_eq!(geometry_vm::TARGETS.len(), 3);
    let mut targets: Vec<&str> = geometry_vm::TARGETS.iter().map(|(name, _)| *name).collect();
    targets.sort_unstable();
    let mut expected = geometry::TABLES.to_vec();
    expected.sort_unstable();
    assert_eq!(targets, expected);
}

#[test]
fn empty_shapes_have_the_placeholder_coordinate() {
    if !reference(geometry::COLLISION).exists() {
        eprintln!("skipped: needs the prepared copies");
        return;
    }
    let table = geometry::read_table(&reference(geometry::COLLISION)).expect("the table");
    let empty = table.shapes.iter().find(|shape| shape.canonical == 1);
    let Some(empty) = empty else {
        panic!("the dictionary has an empty shape");
    };
    // `VoxelShape.getCoords` of `Shapes.empty()` is a single 0 per axis.
    assert_eq!(empty.x, vec![0.0]);
    assert_eq!(empty.y, vec![0.0]);
    assert_eq!(empty.z, vec![0.0]);
}
