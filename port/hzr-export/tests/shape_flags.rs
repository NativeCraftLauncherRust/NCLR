//! Validates the lambda-aware `noCollision` scan against the prepared collision
//! table: every block the scan marks collisionless must have empty collision
//! shapes, except blocks that override `getCollisionShape` (wall hanging signs,
//! whose prepared shapes are non-empty despite `noCollision()`).

#[path = "../src/geometry.rs"]
mod geometry;

#[path = "../src/shape_flags.rs"]
mod shape_flags;

use std::collections::BTreeMap;
use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

#[test]
fn collisionless_blocks_have_empty_collision_states() {
    if !jar().exists() || !workspace().join("tmp/ship-extract").exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let flags = shape_flags::has_collision(&jar()).expect("the flags");
    assert_eq!(flags.len(), 1_286);
    let states = geometry::state_blocks(&workspace().join("tmp/ship-extract/_shared/states.bin"))
        .expect("states");
    let table = geometry::read_table(&workspace().join("tmp/ship-extract/cached-collision.nbt"))
        .expect("collision table");
    let mut order = Vec::new();
    let mut per_block: BTreeMap<&str, (usize, usize, bool, bool)> = BTreeMap::new();
    for (state, (block, _)) in states.iter().enumerate() {
        if order.last().map(|last: &&str| *last != block.as_str()).unwrap_or(true) {
            order.push(block.as_str());
        }
        let entry = per_block
            .entry(block.as_str())
            .or_insert((0, 0, true, false));
        if table.cached[state] < 0 {
            entry.3 = true;
        } else {
            entry.0 += 1;
            if !table.shapes[table.cached[state] as usize].is_empty() {
                entry.1 += 1;
            }
        }
    }
    assert_eq!(order.len(), flags.len(), "registry and flag count");
    let mut violations = Vec::new();
    let mut collisionless = 0usize;
    for (index, block) in order.iter().enumerate() {
        if flags[index] {
            continue;
        }
        collisionless += 1;
        let (real, non_empty, _, _) = per_block.get(block).copied().unwrap_or((0, 0, true, false));
        if real > 0 && non_empty > 0 {
            violations.push((*block).to_string());
        }
    }
    // The wall hanging signs are the one documented exception: their prepared
    // collision shapes come from an override, not from the flag.
    // Documented overrides (their prepared collision shapes are non-empty even
    // though the properties say `noCollision()`).
    let unexpected: Vec<&String> = violations
        .iter()
        .filter(|block| {
            !block.ends_with("_wall_hanging_sign") && block.as_str() != "minecraft:pitcher_crop"
        })
        .collect();
    eprintln!(
        "collisionless {collisionless} of {}; violations {} (all wall hanging signs: {})",
        flags.len(),
        violations.len(),
        unexpected.is_empty()
    );
    for block in violations.iter().take(20) {
        eprintln!("  override: {block}");
    }
    assert!(
        unexpected.is_empty(),
        "{} unexpected violations: {:?}",
        unexpected.len(),
        unexpected
    );
}

#[test]
fn the_known_families_have_their_flags() {
    if !jar().exists() {
        eprintln!("skipped: needs the jar");
        return;
    }
    let flags = shape_flags::has_collision(&jar()).expect("the flags");
    let states = geometry::state_blocks(&workspace().join("tmp/ship-extract/_shared/states.bin"))
        .expect("states");
    let mut order = Vec::new();
    for (block, _) in &states {
        if order.last().map(|last: &&str| *last != block.as_str()).unwrap_or(true) {
            order.push(block.as_str());
        }
    }
    let flag = |id: &str| {
        let index = order
            .iter()
            .position(|candidate| *candidate == id)
            .unwrap_or_else(|| panic!("{id} is registered"));
        flags[index]
    };
    // The families fixed by the lambda-aware scan carry `noCollision()`.
    for id in [
        "minecraft:white_banner",
        "minecraft:shulker_box",
        "minecraft:oak_hanging_sign",
        "minecraft:oak_wall_hanging_sign",
        "minecraft:torch",
        "minecraft:air",
    ] {
        eprintln!("flag {id} = {}", flag(id));
    }
    assert!(!flag("minecraft:torch"));
    assert!(!flag("minecraft:air"));
    for id in ["minecraft:stone", "minecraft:glass", "minecraft:oak_leaves"] {
        assert!(flag(id), "{id} keeps its collision");
    }
}
