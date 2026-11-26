//! Do the block-behaviours signal columns come out of the jar exactly?
//!
//! The oracle is `tmp/ship-extract/block-behaviours.json#states`, the prepared
//! copy of `VanillaBlockBehaviourExporter.states()`: one 9-column row per
//! registry state. This test checks the four signal-related columns:
//!
//! * column 1 (`flags`) bit 8 = `state.isSignalSource()`;
//! * column 5 `weak` and column 6 `strong`: 4 bits per `Direction.ordinal()`,
//!   taken over `EmptyBlockGetter.INSTANCE` at `BlockPos.ZERO`;
//! * column 7 `signalError`.
//!
//! The inputs are rebuilt from the jar, not from the prepared copy:
//! `block::registry` gives each block's class chain and `states::state_table`
//! the per-state property values, in registry order.

#[path = "../src/behaviour_signal.rs"]
mod behaviour_signal;
#[path = "../src/block.rs"]
mod block;
#[path = "../src/states.rs"]
mod states;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::Value;

use behaviour_signal::{rule_for, signal_row, Rule};

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn prepared() -> PathBuf {
    workspace().join("tmp/ship-extract/block-behaviours.json")
}

/// One state in registry order, with everything the signal columns need.
struct Row {
    block: String,
    chain: Vec<String>,
    properties: Vec<(String, String)>,
}

/// `Lists.cartesianProduct` in `StateDefinition` order: sorted property names,
/// first property varies slowest.
fn cartesian(properties: &[states::Property]) -> Vec<Vec<(String, String)>> {
    let mut out: Vec<Vec<(String, String)>> = vec![Vec::new()];
    for property in properties {
        let mut next = Vec::with_capacity(out.len() * property.values.len());
        for prefix in &out {
            for value in &property.values {
                let mut row = prefix.clone();
                row.push((property.name.clone(), value.clone()));
                next.push(row);
            }
        }
        out = next;
    }
    out
}

/// Every state of every block, in registry order (built once per test binary;
/// `block::registry` + `states::state_table` read the whole jar).
fn rows() -> &'static [Row] {
    static ROWS: OnceLock<Vec<Row>> = OnceLock::new();
    ROWS.get_or_init(state_rows).as_slice()
}

fn state_rows() -> Vec<Row> {
    let table = states::state_table(&jar()).expect("the state table");
    let registry = block::registry(&jar()).expect("the block registry");
    assert_eq!(table.blocks.len(), registry.len(), "block count");
    let mut rows = Vec::with_capacity(table.state_count());
    for (block, registered) in table.blocks.iter().zip(registry.iter()) {
        assert_eq!(block.id, registered.id, "registry order");
        let sorted = block.sorted();
        let combos = cartesian(
            &sorted
                .iter()
                .map(|property| (*property).clone())
                .collect::<Vec<_>>(),
        );
        assert_eq!(combos.len(), block.state_count, "{} state count", block.id);
        for combo in combos {
            rows.push(Row {
                block: block.id.clone(),
                chain: registered.chain.clone(),
                properties: combo,
            });
        }
    }
    rows
}

/// The prepared `block-behaviours.json` as `(blocks, states)`.
fn prepared_tables() -> (Vec<Value>, Vec<Value>) {
    let text = std::fs::read_to_string(prepared()).expect("the prepared block behaviours");
    let root: Value = serde_json::from_str(&text).expect("block-behaviours.json");
    let blocks = root
        .get("blocks")
        .and_then(Value::as_array)
        .cloned()
        .expect("blocks array");
    let states = root
        .get("states")
        .and_then(Value::as_array)
        .cloned()
        .expect("states array");
    (blocks, states)
}

fn column(row: &Value, index: usize) -> i64 {
    row.get(index)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("column {index} is not an integer: {row}"))
}

fn boolean_column(row: &Value, index: usize) -> bool {
    row.get(index)
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("column {index} is not a bool: {row}"))
}

/// One rule's coverage: source states, blocks and classes.
#[derive(Default)]
struct Stats {
    states: usize,
    blocks: BTreeSet<String>,
}

#[test]
fn every_state_matches_the_prepared_signal_columns() {
    if !jar().exists() || !prepared().exists() {
        eprintln!("skipped: needs the jar and the prepared copy");
        return;
    }
    let rows = rows();
    let (prepared_blocks, prepared_states) = prepared_tables();
    assert_eq!(rows.len(), prepared_states.len(), "state count");

    // The prepared blocks array and the prepared state rows share the registry
    // order the jar tables use; check it block by block before indexing rows.
    let mut cursor = 0usize;
    for wanted in &prepared_blocks {
        let name = wanted
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("a prepared block has no name: {wanted}"));
        let start = cursor;
        while cursor < rows.len() && rows[cursor].block == name {
            cursor += 1;
        }
        assert!(cursor > start, "no states for prepared block {name}");
    }
    assert_eq!(cursor, rows.len(), "registry order");

    let mut source_mismatches = 0usize;
    let mut weak_mismatches = 0usize;
    let mut strong_mismatches = 0usize;
    let mut error_mismatches = 0usize;
    let mut samples: Vec<String> = Vec::new();
    let mut source_states = 0usize;
    let mut source_blocks: BTreeSet<String> = BTreeSet::new();
    let mut gaps: BTreeSet<String> = BTreeSet::new();
    let mut stats: BTreeMap<&'static str, Stats> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        let wanted = &prepared_states[index];
        let wanted_source = column(wanted, 1) & 8 != 0;
        let wanted_weak = column(wanted, 5);
        let wanted_strong = column(wanted, 6);
        let wanted_error = boolean_column(wanted, 7);
        let values: BTreeMap<String, String> = row.properties.iter().cloned().collect();
        let actual = signal_row(&row.chain, &values);
        if actual.source != wanted_source {
            source_mismatches += 1;
            if samples.len() < 12 {
                samples.push(format!(
                    "state {index} {}: source {} expected {wanted_source}",
                    row.block, actual.source
                ));
            }
        }
        if actual.weak != wanted_weak {
            weak_mismatches += 1;
            if samples.len() < 12 {
                samples.push(format!(
                    "state {index} {}: weak {} expected {wanted_weak}",
                    row.block, actual.weak
                ));
            }
        }
        if actual.strong != wanted_strong {
            strong_mismatches += 1;
            if samples.len() < 12 {
                samples.push(format!(
                    "state {index} {}: strong {} expected {wanted_strong}",
                    row.block, actual.strong
                ));
            }
        }
        if actual.error != wanted_error {
            error_mismatches += 1;
            if samples.len() < 12 {
                samples.push(format!(
                    "state {index} {}: signalError {} expected {wanted_error}",
                    row.block, actual.error
                ));
            }
        }
        if !wanted_source {
            continue;
        }
        source_states += 1;
        source_blocks.insert(row.block.clone());
        match rule_for(&row.chain) {
            Some(rule) => {
                let entry = stats.entry(rule.name()).or_default();
                entry.states += 1;
                entry.blocks.insert(row.block.clone());
            }
            None => {
                gaps.insert(row.block.clone());
            }
        }
    }
    let mut report = String::new();
    for (rule, entry) in &stats {
        report.push_str(&format!(
            "    {rule}: {} states, {} blocks\n",
            entry.states,
            entry.blocks.len()
        ));
    }
    eprintln!(
        "block-behaviours signal coverage ({} states, {source_states} signal-source states over {} blocks):\n  \
         source flag {}/{} exact\n  weak {}/{} exact\n  strong {}/{} exact\n  signalError {}/{} exact\n  \
         rules ({}):\n{report}",
        rows.len(),
        source_blocks.len(),
        rows.len() - source_mismatches,
        rows.len(),
        rows.len() - weak_mismatches,
        rows.len(),
        rows.len() - strong_mismatches,
        rows.len(),
        rows.len() - error_mismatches,
        rows.len(),
        stats.len(),
    );
    assert!(
        samples.is_empty(),
        "signal columns differ; first samples:\n{}",
        samples.join("\n")
    );
    assert_eq!(rows.len(), 35_723, "the prepared table's state count");
    assert_eq!(source_states, 2_647, "signal-source states");
    assert_eq!(source_blocks.len(), 57, "signal-source blocks");
    assert_eq!(stats.len(), Rule::ALL.len(), "every rule is exercised");
    assert!(
        gaps.is_empty(),
        "signal-source blocks without a rule: {gaps:?}"
    );
}

#[test]
fn the_rule_table_covers_every_prepared_signal_class() {
    if !jar().exists() || !prepared().exists() {
        eprintln!("skipped: needs the jar and the prepared copy");
        return;
    }
    let rows = rows();
    let (_, prepared_states) = prepared_tables();
    assert_eq!(rows.len(), prepared_states.len(), "state count");
    let mut classes: BTreeMap<String, (usize, Option<&'static str>)> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        if column(&prepared_states[index], 1) & 8 == 0 {
            continue;
        }
        let class = row.chain.first().cloned().unwrap_or_default();
        let entry = classes.entry(class).or_insert((0, None));
        entry.0 += 1;
        entry.1 = rule_for(&row.chain).map(Rule::name);
        assert!(
            entry.1.is_some(),
            "{} ({}) is a signal source with no rule",
            row.block,
            row.chain.join(" -> ")
        );
    }
    let mut report = String::new();
    let mut missing = 0usize;
    for (class, (states, rule)) in &classes {
        match rule {
            Some(rule) => report.push_str(&format!("    {class} -> {rule}: {states} states\n")),
            None => {
                missing += 1;
                report.push_str(&format!("    {class} -> MISSING: {states} states\n"));
            }
        }
    }
    eprintln!(
        "block-behaviours signal classes ({} concrete classes):\n{report}",
        classes.len()
    );
    assert_eq!(missing, 0, "signal-source classes without a rule");
    assert_eq!(classes.len(), 22, "signal-source concrete classes");
    let rule_count = classes
        .values()
        .filter_map(|(_, rule)| *rule)
        .collect::<BTreeSet<&str>>()
        .len();
    assert_eq!(rule_count, Rule::ALL.len(), "every rule names a class");
}
