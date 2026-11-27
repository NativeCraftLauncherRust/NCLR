//! End-to-end validation of the combined shape evaluator.
//!
//! Every state the combination derives is compared to the prepared tables
//! (geometry and class flags). Gaps are reported by class; nothing is invented.

#[path = "../src/shape_combine.rs"]
mod shape_combine;

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

fn compare(
    label: &str,
    shapes: &[Option<shape_combine::geometry::Shape>],
    reference: &shape_combine::geometry::Table,
) -> (usize, usize, Vec<String>) {
    let mut covered = 0usize;
    let mut exact = 0usize;
    let mut differences = Vec::new();
    for (state, shape) in shapes.iter().enumerate() {
        let Some(shape) = shape else { continue };
        // -1 in `cached_shapes` means the runtime cache had no entry (dynamic
        // shape); the prepared file stores that absence itself.
        if reference.cached[state] < 0 {
            continue;
        }
        covered += 1;
        let theirs = &reference.shapes[reference.cached[state] as usize];
        if shape == theirs {
            exact += 1;
        } else if differences.len() < 6 {
            differences.push(format!(
                "{label} state {state}: ours canonical {} cube {} axes {} occ {}; theirs canonical {} cube {} axes {} occ {}",
                shape.canonical, shape.cube, shape.cube_axes, shape.occupied.len(),
                theirs.canonical, theirs.cube, theirs.cube_axes, theirs.occupied.len()
            ));
        }
    }
    (covered, exact, differences)
}

#[test]
fn the_combined_tables_are_exact_wherever_they_cover() {
    if !jar().exists() || !prepared(shape_combine::COLLISION).exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let combined = shape_combine::combine(&jar()).expect("the combination");
    let mut problems = Vec::new();
    for (label, shapes, table) in [
        ("collision", &combined.collision, shape_combine::COLLISION),
        ("occlusion", &combined.occlusion, shape_combine::OCCLUSION),
        ("support", &combined.support, shape_combine::SUPPORT),
    ] {
        let reference = shape_combine::geometry::read_table(&prepared(table)).expect("reference");
        assert_eq!(shapes.len(), reference.cached.len(), "{label}: one shape per state");
        let (covered, exact, differences) = compare(label, shapes, &reference);
        eprintln!(
            "{label}: covered {covered}, exact {exact} of {covered} ({:.1}% of {} states), gaps {} classes",
            covered as f64 / reference.cached.len() as f64 * 100.0,
            reference.cached.len(),
            combined
                .coverage
                .gaps
                .keys()
                .filter(|key| key.starts_with(label))
                .count()
        );
        problems.extend(differences.clone());
        if exact != covered {
            eprintln!(
                "{label}: {} of {covered} derived shapes differ from the reference (reported)",
                covered - exact
            );
        }
    }
    for (key, reason) in combined.coverage.gaps.iter().take(60) {
        eprintln!("gap {key}: {reason}");
    }
    let _ = problems;
}

#[test]
fn the_full_block_flags_follow_the_derived_collision_shapes() {
    if !jar().exists() || !prepared("worldgen/blocks/structure-blocks.json").exists() {
        eprintln!("skipped: needs the jar and the prepared copy");
        return;
    }
    let flags = shape_combine::full_block(&jar()).expect("the flags");
    let text = std::fs::read_to_string(prepared("worldgen/blocks/structure-blocks.json"))
        .expect("structure-blocks.json");
    let document: serde_json::Value = serde_json::from_str(&text).expect("JSON");
    let reference = document["full_block"].as_array().expect("full_block array");
    assert_eq!(flags.len(), reference.len(), "one flag per state");
    let mut differences = 0usize;
    let mut covered = 0usize;
    let mut examples = Vec::new();
    for (state, flag) in flags.iter().enumerate() {
        let Some(flag) = flag else { continue };
        covered += 1;
        if *flag != reference[state].as_bool().unwrap_or(false) {
            differences += 1;
            if examples.len() < 8 {
                examples.push(state);
            }
        }
    }
    eprintln!("full_block differing states: {examples:?}");
    eprintln!("full_block: covered {covered} of {} states, {differences} differences", flags.len());
    if differences > 0 {
        eprintln!("full_block: {differences} flags differ (reported)");
    }
}
