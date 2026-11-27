//! Does the sturdy bitmask column of `block-behaviours.json#states` reproduce?
//!
//! The reference is `tmp/ship-extract/block-behaviours.json`: one row per
//! registry state, whose first element is the 18-bit `SupportType x Direction`
//! mask. Every derived mask is compared, including the 199 states of
//! `dynamicShape()` blocks that the shape tables leave uncached.

#[path = "../src/behaviour_sturdy.rs"]
mod behaviour_sturdy;

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

/// Derive once; the shape evaluation is the expensive part and several tests
/// inspect the same table.
fn derived() -> &'static (Vec<Option<i64>>, behaviour_sturdy::Coverage) {
    static CACHE: std::sync::OnceLock<(Vec<Option<i64>>, behaviour_sturdy::Coverage)> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| behaviour_sturdy::sturdy_masks(&jar()).expect("the sturdy masks"))
}

fn reference_sturdy() -> Vec<i64> {
    let text = std::fs::read_to_string(prepared(behaviour_sturdy::BEHAVIOUR_TABLE))
        .expect("block-behaviours.json");
    let document: serde_json::Value = serde_json::from_str(&text).expect("JSON");
    let rows = document["states"].as_array().expect("states array");
    rows.iter()
        .map(|row| {
            row.as_array()
                .and_then(|columns| columns.first())
                .and_then(serde_json::Value::as_i64)
                .expect("a sturdy mask")
        })
        .collect()
}

#[test]
fn every_derived_sturdy_mask_matches_the_prepared_table() {
    if !jar().exists() || !prepared(behaviour_sturdy::BEHAVIOUR_TABLE).exists() {
        eprintln!("skipped: needs the jar and the prepared copy");
        return;
    }
    let (masks, coverage) = derived();
    let reference = reference_sturdy();
    assert_eq!(masks.len(), reference.len(), "one mask per registry state");
    assert_eq!(coverage.states, reference.len(), "coverage counts every state");
    let mut compared = 0usize;
    let mut exact = 0usize;
    let mut differences = Vec::new();
    for (state, mask) in masks.iter().enumerate() {
        let Some(mask) = mask else { continue };
        compared += 1;
        if *mask == reference[state] {
            exact += 1;
        } else if differences.len() < 15 {
            differences.push(format!(
                "state {state}: ours {mask:#08x} ({}), theirs {:08x} ({})",
                decode(*mask),
                reference[state],
                decode(reference[state])
            ));
        }
    }
    eprintln!(
        "sturdy: {exact} of {compared} exact, {} derived of {} states, {} dynamic filled, {} gaps",
        coverage.derived, coverage.states, coverage.dynamic_filled, coverage.missing
    );
    for (class, reason) in coverage.gaps.iter().take(12) {
        eprintln!("  gap {class}: {reason}");
    }
    for difference in &differences {
        eprintln!("  {difference}");
    }
    assert_eq!(
        exact, compared,
        "{} of {compared} derived sturdy masks differ",
        compared - exact
    );
}

#[test]
fn all_35723_states_are_derived_including_the_dynamic_ones() {
    if !jar().exists() || !prepared(behaviour_sturdy::BEHAVIOUR_TABLE).exists() {
        eprintln!("skipped: needs the jar and the prepared copy");
        return;
    }
    let (masks, coverage) = derived();
    assert_eq!(coverage.states, 35_723);
    assert_eq!(coverage.derived, coverage.states, "every state has a mask");
    assert_eq!(coverage.missing, 0, "no gaps");
    assert_eq!(
        coverage.dynamic_filled, 199,
        "the dynamicShape blocks are exactly 199 states"
    );
    assert!(coverage.gaps.is_empty());
    assert_eq!(masks.iter().filter(|mask| mask.is_some()).count(), 35_723);
}

#[test]
fn known_states_have_the_expected_masks() {
    if !jar().exists() || !prepared(behaviour_sturdy::BEHAVIOUR_TABLE).exists() {
        eprintln!("skipped: needs the jar and the prepared copy");
        return;
    }
    let (masks, _) = derived();
    let reference = reference_sturdy();
    // The derived masks equal the reference everywhere the test above passed;
    // these are the readable sanity cases.
    let mut found_snow = false;
    let mut found_scaffolding = false;
    let mut found_shulker = false;
    let mut found_empty_collision = false;
    let mut found_full_block = false;
    for (state, mask) in masks.iter().enumerate() {
        let Some(mask) = mask else { continue };
        assert_eq!(*mask, reference[state], "state {state}");
        // Full blocks: Shapes.block() support covers all 18 bits.
        if *mask == 0x3ffff {
            found_full_block = true;
        }
        // States with `Shapes.empty()` support have no bits.
        if *mask == 0 {
            found_empty_collision = true;
        }
        // 8322 = FULL UP | CENTER UP | RIGID UP (scaffolding's top plate).
        if *mask == 8322 {
            found_scaffolding = true;
        }
        // 4161 = DOWN for FULL/CENTER/RIGID (a snow layer's bottom face).
        if *mask == 4161 {
            found_snow = true;
        }
        // 262143 for the closed shulker boxes.
        if *mask == 262143 {
            found_shulker = true;
        }
    }
    assert!(found_full_block && found_empty_collision, "extreme masks");
    assert!(found_snow, "snow layer masks");
    assert!(found_scaffolding, "scaffolding mask");
    assert!(found_shulker, "shulker box mask");
}

/// Decode a mask to `support/direction` names for the difference report.
fn decode(mask: i64) -> String {
    let mut parts = Vec::new();
    for support in 0..3 {
        for direction in 0..6 {
            if mask & (1 << (support * 6 + direction)) != 0 {
                parts.push(format!(
                    "{}/{}",
                    behaviour_sturdy::SUPPORTS[support],
                    behaviour_sturdy::DIRECTIONS[direction]
                ));
            }
        }
    }
    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(" ")
    }
}


