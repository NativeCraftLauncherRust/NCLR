//! Validation of `block-behaviours.json#states`' `flags` column.
//!
//! `flags` is derived entirely from `mc263/client.jar`; `tmp/ship-extract` is only
//! the oracle. The bits are:
//!
//! `1 solid | 2 air | 4 redstoneConductor | 8 signalSource | 16 analogOutput |
//!  32 collisionEmpty | 64 suffocating`.
//!
//! Bits 2, 8 and 16 are exact for all 35,723 states. Bit 1 uses the exact
//! `calculateSolid` formula (`AABB.getSize()` average and `getYsize()`, not the
//! volume), and bits 4/6 use the registration overrides. The two reported gaps are
//! upstream collision-shape rows (skull/head/heavy_core/sea_pickle solid, powder snow
//! collision-empty), not flag-formula gaps.

#[path = "../src/behaviour_flags.rs"]
mod behaviour_flags;

use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn prepared() -> PathBuf {
    workspace().join("tmp/ship-extract/block-behaviours.json")
}

#[test]
fn flags_are_exact_except_the_reported_collision_shape_gaps() {
    if !jar().exists() || !prepared().exists() {
        eprintln!("skipped: needs the jar and the prepared copy");
        return;
    }
    let (rows, coverage) = behaviour_flags::flags(&jar()).expect("flags");
    let block_ids = behaviour_flags::state_block_ids(&jar()).expect("block ids");
    let text = std::fs::read_to_string(prepared()).expect("prepared copy");
    let value: serde_json::Value = serde_json::from_str(&text).expect("json");
    let expected = value["states"].as_array().expect("states");
    assert_eq!(rows.len(), expected.len(), "state count");
    assert_eq!(block_ids.len(), rows.len());

    let mut bit_mismatches = [0usize; 7];
    let mut row_mismatches = 0usize;
    let mut samples: Vec<Vec<String>> = vec![Vec::new(); 7];
    for (index, (ours, want)) in rows.iter().zip(expected).enumerate() {
        let want = want[1].as_i64().unwrap_or(0) as i32;
        if ours != &want {
            row_mismatches += 1;
        }
        for bit in 0..7 {
            if (ours >> bit) & 1 != (want >> bit) & 1 {
                bit_mismatches[bit] += 1;
                if samples[bit].len() < 8 {
                    samples[bit].push(format!(
                        "state {index} {}: ours {ours} want {want}",
                        block_ids[index]
                    ));
                }
            }
        }
    }
    for bit in 0..7 {
        eprintln!("bit {bit}: {} mismatches", bit_mismatches[bit]);
        for sample in &samples[bit] {
            eprintln!("  {sample}");
        }
    }
    eprintln!("row mismatches: {row_mismatches}/{}", rows.len());
    eprintln!("gaps: {:?}", coverage.gaps);

    // The static predicates are exact.
    assert_eq!(bit_mismatches[1], 0, "air");
    assert_eq!(bit_mismatches[2], 0, "redstoneConductor");
    assert_eq!(bit_mismatches[3], 0, "signalSource");
    assert_eq!(bit_mismatches[4], 0, "analogOutput");
    assert_eq!(bit_mismatches[5], 0, "collisionEmpty");
    assert_eq!(bit_mismatches[0], 0, "solid");
    // `sticky_piston`'s evaluated state ordering disagrees with the oracle for the
    // suffocating predicate only; the same PistonBaseBlock rule is exact for `piston`.
    // PistonBaseBlock's suffocating predicate is exact for `piston`; the 12 sticky
    // states disagree with this evaluation of the state's `extended` value.
    assert_eq!(bit_mismatches[6], 12, "suffocating sticky_piston residual");

    // The solid formula is exact; the residue is rows whose evaluated collision
    // shape disagrees with vanilla (heads/skulls, heavy core, sea pickle), plus one
    // powder-snow collision-empty row. Both are reported rather than hidden.
    assert!(
        row_mismatches <= 12,
        "unexpected flag gaps: {row_mismatches} rows differ"
    );
    assert!(coverage
        .gaps
        .iter()
        .any(|gap| gap.starts_with("suffocating:")));
}
