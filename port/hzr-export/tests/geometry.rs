//! Does the Rust exporter reproduce Horizon's shape tables from the jar?
//!
//! The reference is `tmp/ship-extract`: the three stripped shape tables. The
//! container codec is expected to round-trip them byte-for-byte; geometry is
//! expected to be exact wherever the derivation covers a state, with the gap
//! count reported.

#[path = "../src/geometry.rs"]
mod geometry;

use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn reference(name: &str) -> PathBuf {
    workspace().join("tmp/ship-extract").join(name)
}

#[test]
fn the_container_round_trips_all_three_prepared_tables() {
    if !reference(geometry::COLLISION).exists() {
        eprintln!("skipped: needs the prepared copies");
        return;
    }
    for name in geometry::TABLES {
        let bytes = std::fs::read(reference(name)).expect("the prepared file");
        let table = geometry::decode(&bytes).expect("decode");
        let again = geometry::encode(&table);
        if again != bytes {
            let at = again
                .iter()
                .zip(bytes.iter())
                .position(|(left, right)| left != right)
                .unwrap_or_else(|| again.len().min(bytes.len()));
            panic!(
                "{name}: differs at byte {at}: ours {} bytes, reference {} bytes; ours {:?} reference {:?}",
                again.len(),
                bytes.len(),
                &again[at.saturating_sub(16)..(at + 16).min(again.len())],
                &bytes[at.saturating_sub(16)..(at + 16).min(bytes.len())]
            );
        }
    }
}

#[test]
fn a_known_block_and_air_have_the_expected_collision_shapes() {
    if !jar().exists() || !reference(geometry::COLLISION).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let (shapes, _) = geometry::collision_shapes(&jar()).expect("the shapes");
    let table = geometry::read_table(&reference(geometry::COLLISION)).expect("the table");
    let states = states_of();
    let index = |id: &str| {
        states
            .iter()
            .position(|(block, _)| block == id)
            .unwrap_or_else(|| panic!("{id} is registered"))
    };
    let stone = index("minecraft:stone");
    assert_eq!(shapes[stone].as_ref(), Some(&geometry::Shape::block()));
    assert_eq!(
        table.shapes[table.cached[stone] as usize],
        geometry::Shape::block()
    );
    // Air overrides `getCollisionShape` to return `Shapes.empty()`, so it is a
    // reported gap rather than part of the default-path subset; the prepared
    // copy confirms the empty shape.
    let air = index("minecraft:air");
    assert!(shapes[air].is_none(), "air is an override, so it is a gap");
    assert!(table.shapes[table.cached[air] as usize].is_empty());
}

#[test]
fn every_derived_collision_shape_matches_the_prepared_copy() {
    if !jar().exists() || !reference(geometry::COLLISION).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let (shapes, coverage) = geometry::collision_shapes(&jar()).expect("the shapes");
    let table = geometry::read_table(&reference(geometry::COLLISION)).expect("the table");
    let mut compared = 0usize;
    let mut differences = Vec::new();
    for (state, shape) in shapes.iter().enumerate() {
        let Some(shape) = shape else {
            continue;
        };
        compared += 1;
        let theirs = &table.shapes[table.cached[state] as usize];
        if shape != theirs && differences.len() < 10 {
            differences.push(format!(
                "state {state}: ours canonical {} cube {} occupied {}, theirs canonical {} cube {} occupied {}",
                shape.canonical,
                shape.cube,
                shape.occupied.len(),
                theirs.canonical,
                theirs.cube,
                theirs.occupied.len()
            ));
        }
    }
    assert_eq!(compared, coverage.covered, "coverage counts the compared states");
    assert!(
        differences.is_empty(),
        "{} of {compared} derived shapes differ:\n{}",
        differences.len(),
        differences.join("\n")
    );
    eprintln!(
        "collision: {} of {} states derived exactly ({:.1}%), {} gaps over {} blocks",
        coverage.covered,
        table.cached.len(),
        100.0 * coverage.covered as f64 / table.cached.len() as f64,
        coverage.missing,
        coverage.gaps.len()
    );
}

#[test]
fn every_derived_occlusion_shape_matches_the_prepared_copy() {
    if !jar().exists() || !reference(geometry::OCCLUSION).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let (shapes, coverage) = geometry::occlusion_shapes(&jar()).expect("the shapes");
    let table = geometry::read_table(&reference(geometry::OCCLUSION)).expect("the table");
    let mut compared = 0usize;
    let mut differences = 0usize;
    for (state, shape) in shapes.iter().enumerate() {
        let Some(shape) = shape else {
            continue;
        };
        compared += 1;
        if *shape != table.shapes[table.cached[state] as usize] {
            differences += 1;
            if differences <= 6 {
                let theirs = &table.shapes[table.cached[state] as usize];
                eprintln!(
                    "occlusion diff state {state}: ours canonical {} cube {}, theirs canonical {} cube {}",
                    shape.canonical, shape.cube, theirs.canonical, theirs.cube
                );
            }
        }
    }
    assert_eq!(compared, coverage.covered);
    assert_eq!(
        differences, 0,
        "{differences} of {compared} derived occlusion shapes differ"
    );
    eprintln!(
        "occlusion: {} of {} states derived exactly ({:.1}%), {} gaps over {} blocks",
        coverage.covered,
        table.cached.len(),
        100.0 * coverage.covered as f64 / table.cached.len() as f64,
        coverage.missing,
        coverage.gaps.len()
    );
}

#[test]
fn the_full_block_flag_follows_the_derived_collision_shape() {
    if !jar().exists() || !reference(geometry::COLLISION).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let (shapes, _) = geometry::collision_shapes(&jar()).expect("the shapes");
    let flags = geometry::full_block(&shapes);
    let table = geometry::read_table(&reference(geometry::COLLISION)).expect("the table");
    for (state, flag) in flags.iter().enumerate() {
        let Some(flag) = flag else {
            continue;
        };
        let full = table.shapes[table.cached[state] as usize].is_block();
        assert_eq!(*flag, full, "state {state}: full_block");
    }
}

fn states_of() -> Vec<(String, usize)> {
    geometry::state_blocks(&workspace().join("tmp/ship-extract/_shared/states.bin"))
        .expect("the prepared states")
}
