//! Does the Rust exporter reproduce Horizon's per-block movement/properties
//! tables from the jar?
//!
//! The reference is `tmp/ship-extract`: `movement-properties.nbt` and
//! `worldgen/blocks/block-properties.nbt`, the stripped copies `ship.hzr3`
//! carries (their `states`/`occlusion` keys live in `_shared/`).

#[path = "../src/shape.rs"]
mod shape;

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
fn the_two_tables_are_byte_identical_to_the_prepared_copies() {
    if !jar().exists() || !reference(shape::TABLE).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let tables = shape::tables(&jar()).expect("the tables");
    assert_eq!(tables.blocks.len(), 1_286, "one row per block");
    let named = std::env::var_os("HZR_EXPORT_OUT").map(std::path::PathBuf::from);
    let out = named.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("hzr-export-shape-{}", std::process::id()))
    });
    let _ = std::fs::remove_dir_all(&out);
    shape::write(&jar(), &out).expect("write");
    for name in [shape::TABLE, shape::MOVEMENT] {
        let mine = std::fs::read(out.join(name)).expect("our file");
        let expected = std::fs::read(reference(name)).expect("the prepared file");
        if mine != expected {
            let at = mine
                .iter()
                .zip(expected.iter())
                .position(|(left, right)| left != right)
                .unwrap_or_else(|| mine.len().min(expected.len()));
            panic!(
                "{name} differs at byte {at}: ours {} bytes, reference {} bytes; ours {:?} reference {:?}",
                mine.len(),
                expected.len(),
                &mine[at.saturating_sub(24)..(at + 24).min(mine.len())],
                &expected[at.saturating_sub(24)..(at + 24).min(expected.len())],
            );
        }
    }
    if named.is_none() {
        let _ = std::fs::remove_dir_all(&out);
    }
}

#[test]
fn a_few_known_blocks_have_their_expected_movement_values() {
    if !jar().exists() {
        eprintln!("skipped: needs the jar");
        return;
    }
    let tables = shape::tables(&jar()).expect("the tables");
    let row = |id: &str| {
        tables
            .blocks
            .iter()
            .find(|(name, _)| name == id)
            .map(|(_, properties)| properties.clone())
            .unwrap_or_else(|| panic!("{id} is in the registry"))
    };
    let stone = row("minecraft:stone");
    assert_eq!(stone.movement.friction, 0.6);
    assert_eq!(stone.movement.speed_factor, 1.0);
    assert_eq!(stone.bitmask(), 16, "plain occluding block");
    // Honey block slows and softens falls through `Properties`.
    let honey = row("minecraft:honey_block");
    assert_eq!(honey.movement.speed_factor, 0.4);
    assert_eq!(honey.movement.jump_factor, 0.5);
    // Slime bounces and is slippery; a bed is the one that softens falls.
    let slime = row("minecraft:slime_block");
    assert_eq!(slime.movement.bounce_restitution, 1.0);
    assert_eq!(slime.movement.friction, 0.8);
    let bed = row("minecraft:gray_bed");
    assert_eq!(bed.movement.bounce_restitution, 0.75);
    assert_eq!(bed.movement.fall_distance_reduction, 0.5);
    // Fence gates are the class check.
    assert!(row("minecraft:oak_fence_gate").movement.fence_gate);
    assert!(!stone.movement.fence_gate);
    // Water is liquid and not occluding.
    let water = row("minecraft:water");
    assert!(water.liquid);
    assert!(!water.can_occlude);
    // liquid + replaceable\n    assert_eq!(water.bitmask(), 12);
}
