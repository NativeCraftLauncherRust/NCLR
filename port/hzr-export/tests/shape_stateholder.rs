//! Does the property-key-backed synthetic state reproduce `StateHolder`'s
//! answers, and does it derive the multiface family's 384 occlusion states?
//!
//! The reference is `tmp/ship-extract/occlusion.nbt` for
//! `GlowLichenBlock`/`MultifaceBlock`/`SculkVeinBlock` (3 classes x 128 states:
//! six face booleans + waterlogged). The step-2b bytecode VM reports these as
//! its `StateHolder.getValues()/propertyKeys()` gap; this module supplies the
//! two arrays and the accessors, and the test checks every state exactly.

#[path = "../src/shape_stateholder.rs"]
mod shape_stateholder;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use shape_stateholder::geometry as table;
use shape_stateholder::{block, states, SyntheticState, StateValue};

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn prepared(name: &str) -> PathBuf {
    workspace().join("tmp/ship-extract").join(name)
}

fn reference(name: &str) -> table::Table {
    table::read_table(&prepared(name)).expect("the prepared table")
}

#[test]
fn the_multiface_family_is_exactly_384_occlusion_states() {
    if !jar().exists() || !prepared(shape_stateholder::geometry::OCCLUSION).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let (shapes, coverage) =
        shape_stateholder::multiface_occlusion_table(&jar()).expect("the table");
    let expected: BTreeSet<String> = shape_stateholder::MULTIFACE_CLASSES
        .iter()
        .map(|class| class.to_string())
        .collect();
    assert_eq!(coverage.states, 384, "3 classes x 128 states");
    assert_eq!(coverage.classes, expected, "the three multiface classes");
    for class in shape_stateholder::MULTIFACE_CLASSES {
        assert_eq!(
            coverage.per_class.get(class).copied(),
            Some(128),
            "{class} should contribute 128 states"
        );
    }
    let derived = shapes.iter().filter(|shape| shape.is_some()).count();
    assert_eq!(derived, 384, "only the multiface states get a shape");
}

#[test]
fn every_derived_multiface_occlusion_shape_is_exact() {
    if !jar().exists() || !prepared(shape_stateholder::geometry::OCCLUSION).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let (shapes, coverage) =
        shape_stateholder::multiface_occlusion_table(&jar()).expect("the table");
    let reference = reference(shape_stateholder::geometry::OCCLUSION);
    let mut compared = 0usize;
    let mut exact = 0usize;
    let mut differences = Vec::new();
    for (state, shape) in shapes.iter().enumerate() {
        let Some(shape) = shape else { continue };
        compared += 1;
        let theirs = &reference.shapes[reference.cached[state] as usize];
        if shape == theirs {
            exact += 1;
        } else if differences.len() < 10 {
            differences.push(format!(
                "state {state}: ours canonical {} cube {} axes {} x{:?} y{:?} z{:?}; theirs canonical {} cube {} axes {} x{:?} y{:?} z{:?}",
                shape.canonical,
                shape.cube,
                shape.cube_axes,
                shape.x,
                shape.y,
                shape.z,
                theirs.canonical,
                theirs.cube,
                theirs.cube_axes,
                theirs.x,
                theirs.y,
                theirs.z
            ));
        }
    }
    eprintln!("multiface occlusion: {exact} of {compared} exact ({} derived)", coverage.states);
    for difference in &differences {
        eprintln!("  {difference}");
    }
    assert_eq!(compared, coverage.states);
    assert_eq!(exact, compared, "{} states differ", compared - exact);
}

#[test]
fn the_synthetic_state_answers_every_state_holder_query() {
    if !jar().exists() {
        eprintln!("skipped: needs the jar");
        return;
    }
    let registry = block::registry(&jar()).expect("registry");
    let table = states::state_table(&jar()).expect("state table");
    let mut checked = 0usize;
    for (index, row) in table.blocks.iter().enumerate() {
        if !shape_stateholder::is_multiface_class(&registry[index].class) {
            continue;
        }
        for state_index in 0..row.state_count {
            let state = SyntheticState::from_index(row, state_index).expect("state");
            checked += 1;
            // Six face booleans plus waterlogged, in createBlockStateDefinition
            // order; every key answers the identity scan.
            assert_eq!(state.property_keys().len(), 7, "{}", row.id);
            assert_eq!(state.property_values().len(), 7);
            assert!(!state.is_singleton_state());
            for key in state.property_keys() {
                let slot = state
                    .value_index(&key.name)
                    .unwrap_or_else(|| panic!("{} has no slot for {}", row.id, key.name));
                assert_eq!(key.ordinal, slot);
                assert!(state.has_property(&key.name));
                assert!(state.get_value(&key.name).is_some());
                assert_eq!(
                    state.get_optional_value(&key.name),
                    state.get_value(&key.name)
                );
                assert_eq!(
                    state.get_value_or_else(&key.name, &StateValue::Bool(false)),
                    state.get_value(&key.name).expect("value")
                );
            }
            let pairs = state.get_values();
            assert_eq!(pairs.len(), 7);
            for (key, value) in &pairs {
                assert_eq!(state.value_index(&key.name), Some(key.ordinal));
                assert_eq!(*value, state.get_value(&key.name).expect("value"));
            }
            // The VM-facing constructor round-trips the serialized values.
            let serialized: BTreeMap<String, String> = pairs
                .iter()
                .map(|(key, value)| (key.name.clone(), value.serialized()))
                .collect();
            let rebuilt = SyntheticState::from_values(row, &serialized).expect("from_values");
            assert_eq!(rebuilt, state, "{} state {state_index}", row.id);
            // `setValue` only replaces that slot.
            let north = state.get_value("north").cloned().expect("north");
            let toggled = state
                .set_value("north", StateValue::Bool(!north.as_bool().expect("bool")))
                .expect("setValue");
            assert_ne!(toggled, state);
            assert_eq!(toggled.get_value("south"), state.get_value("south"));
        }
    }
    assert_eq!(checked, 384);
    eprintln!("synthetic state: {checked} states answer the StateHolder queries");
}

#[test]
fn the_all_false_state_is_the_full_block_and_single_faces_are_plates() {
    if !jar().exists() || !prepared(shape_stateholder::geometry::OCCLUSION).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let (shapes, _) =
        shape_stateholder::multiface_occlusion_table(&jar()).expect("the table");
    let registry = block::registry(&jar()).expect("registry");
    let table = states::state_table(&jar()).expect("state table");
    let mut index = 0usize;
    let mut saw_empty_union = 0usize;
    let mut saw_north_only = 0usize;
    let mut saw_up_only = 0usize;
    for (block_index, row) in table.blocks.iter().enumerate() {
        if !shape_stateholder::is_multiface_class(&registry[block_index].class) {
            index += row.state_count;
            continue;
        }
        for state_index in 0..row.state_count {
            let state = SyntheticState::from_index(row, state_index).expect("state");
            let faces: Vec<bool> = shape_stateholder::DIRECTIONS
                .iter()
                .map(|direction| {
                    state
                        .get_value(direction)
                        .and_then(StateValue::as_bool)
                        .expect("face boolean")
                })
                .collect();
            let set = faces.iter().filter(|set| **set).count();
            let shape = shapes[index].as_ref().expect("derived");
            if set == 0 {
                saw_empty_union += 1;
                assert_eq!(shape.canonical, 2, "empty union is Shapes.block()");
            } else if set == 1 {
                match shape_stateholder::DIRECTIONS[faces
                    .iter()
                    .position(|set| *set)
                    .expect("one face")]
                {
                    "north" => {
                        saw_north_only += 1;
                        assert_eq!(shape.x, vec![0.0, 1.0]);
                        assert_eq!(shape.y, vec![0.0, 1.0]);
                        assert_eq!(shape.z, vec![0.0, 0.0625], "1/16 plate");
                    }
                    "up" => {
                        saw_up_only += 1;
                        assert_eq!(shape.y, vec![0.9375, 1.0], "top plate");
                    }
                    _ => {}
                }
            }
            index += 1;
        }
    }
    assert!(saw_empty_union > 0 && saw_north_only > 0 && saw_up_only > 0);
    eprintln!(
        "example states: {saw_empty_union} empty unions, {saw_north_only} north-only, {saw_up_only} up-only"
    );
}

#[test]
fn the_collision_table_confirms_these_states_have_no_collision() {
    if !jar().exists() || !prepared(shape_stateholder::geometry::COLLISION).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    // The family is `noCollision()`; the prepared collision table is empty for
    // every one of the 384 states, which is why only the occlusion table needed
    // this state model.
    let (shapes, coverage) =
        shape_stateholder::multiface_occlusion_table(&jar()).expect("the table");
    let collision = reference(shape_stateholder::geometry::COLLISION);
    let mut empty = 0usize;
    for (state, shape) in shapes.iter().enumerate() {
        if shape.is_none() {
            continue;
        }
        assert_eq!(
            collision.shapes[collision.cached[state] as usize].canonical,
            1,
            "state {state} should have an empty collision shape"
        );
        empty += 1;
    }
    assert_eq!(empty, coverage.states);
    eprintln!("collision: all {empty} multiface states are empty as expected");
}



