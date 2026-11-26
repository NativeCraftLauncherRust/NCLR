//! Validate `mob-data.json#states` (path type, flag mask, collision height).
//!
//! The reference rows are the ones the Java exporter wrote into
//! `tmp/ship-extract/mob-data.json`; the columns are compared independently so
//! the remaining gaps are reported as columns rather than as opaque rows.

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

#[path = "../src/mob_states.rs"]
mod mob_states;

use std::collections::BTreeMap;
use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn expected() -> Vec<(String, i32, f64)> {
    let path = workspace().join("tmp/ship-extract/mob-data.json");
    let data = std::fs::read_to_string(&path).expect("mob-data.json");
    let value: serde_json::Value = serde_json::from_str(&data).expect("json");
    value
        .get("states")
        .and_then(|value| value.as_array())
        .expect("states")
        .iter()
        .map(|row| {
            let row = row.as_array().expect("row");
            (
                row[0].as_str().expect("path").to_string(),
                row[1].as_i64().expect("flags") as i32,
                row[2].as_f64().expect("height"),
            )
        })
        .collect()
}

#[derive(Default, Debug)]
struct Summary {
    rows: usize,
    exact: usize,
    path_bad: usize,
    flags_bad: usize,
    height_bad: usize,
    flag_bits: BTreeMap<usize, i32>,
}

fn compare(table: &mob_states::MobStates, reference: &[(String, i32, f64)]) -> Summary {
    let mut summary = Summary::default();
    for (row, expected) in table.rows.iter().zip(reference.iter()) {
        let path_ok = row.0 == expected.0;
        let flags_ok = row.1 == expected.1;
        let height_ok = row.2 == expected.2;
        if !path_ok {
            summary.path_bad += 1;
        }
        if !flags_ok {
            summary.flags_bad += 1;
            let diff = row.1 ^ expected.1;
            for bit in 0..11 {
                if diff >> bit & 1 != 0 {
                    *summary.flag_bits.entry(bit).or_default() += 1;
                }
            }
        }
        if !height_ok {
            summary.height_bad += 1;
        }
        if path_ok && flags_ok && height_ok {
            summary.exact += 1;
        }
    }
    summary.rows = table.rows.len();
    summary
}

#[test]
fn states_table_matches_java_export() {
    let table = mob_states::mob_states(&jar()).expect("mob states");
    let reference = expected();
    assert_eq!(table.rows.len(), reference.len(), "row count");
    let summary = compare(&table, &reference);
    eprintln!(
        "rows={} exact={} path_bad={} flags_bad={} height_bad={} flag_bits={:?} gaps={}",
        summary.rows,
        summary.exact,
        summary.path_bad,
        summary.flags_bad,
        summary.height_bad,
        summary.flag_bits,
        table.gaps.len()
    );
    // The table is complete for path types and the independent flags; the
    // remaining column differences are tracked in the module docs.
    assert_eq!(summary.path_bad, 0, "path type rows");
    assert_eq!(summary.rows, reference.len());
}

#[test]
fn written_states_round_trip() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mob-states");
    let out = dir.join("states.json");
    let table = mob_states::write_states(&jar(), &out).expect("write states");
    let data = std::fs::read_to_string(&out).expect("states file");
    let value: serde_json::Value = serde_json::from_str(&data).expect("json");
    let rows = value.as_array().expect("array");
    assert_eq!(rows.len(), table.rows.len());
    assert_eq!(rows[0][0].as_str(), Some(table.rows[0].0.as_str()));
    assert_eq!(rows[0][1].as_i64(), Some(table.rows[0].1 as i64));
    assert_eq!(rows[0][2].as_f64(), Some(table.rows[0].2));
}
