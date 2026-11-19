//! How many states' collision shape is empty, a full cube, or something else?
//!
//!   cargo run --offline --example nbt_coverage -- <file.nbt>

use hzr_fetch::nbt_value::{self, Tag};
use std::collections::BTreeMap;
use std::path::Path;

fn byte(tag: Option<&Tag>) -> i64 {
    match tag {
        Some(Tag::Byte(value)) => *value as i64,
        Some(Tag::Int(value)) => *value as i64,
        _ => 0,
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("an .nbt file");
    let bytes = std::fs::read(Path::new(&path)).unwrap();
    let (_, root) = nbt_value::parse(&bytes).expect("a table");
    let shapes = match root.get("shapes") {
        Some(Tag::List { items, .. }) => items.clone(),
        _ => Vec::new(),
    };
    let cached = match root.get("cached_shapes") {
        Some(Tag::IntArray(values)) => values.clone(),
        _ => Vec::new(),
    };

    // canonical: 1 = empty, 2 = the full cube, 0 = anything else.
    let mut by_kind: BTreeMap<i64, usize> = BTreeMap::new();
    let mut by_shape: BTreeMap<usize, usize> = BTreeMap::new();
    for index in &cached {
        let id = *index as usize;
        let Some(shape) = shapes.get(id) else { continue };
        *by_kind.entry(byte(shape.get("canonical"))).or_default() += 1;
        *by_shape.entry(id).or_default() += 1;
    }

    let total = cached.len();
    let empty = by_kind.get(&1).copied().unwrap_or(0);
    let cube = by_kind.get(&2).copied().unwrap_or(0);
    println!("{path:?}");
    println!("states        {total}");
    println!("shapes        {}", shapes.len());
    println!(
        "empty         {empty} ({:.1}%)",
        100.0 * empty as f64 / total as f64
    );
    println!(
        "full cube     {cube} ({:.1}%)",
        100.0 * cube as f64 / total as f64
    );
    println!(
        "other         {} ({:.1}%)",
        total - empty - cube,
        100.0 * (total - empty - cube) as f64 / total as f64
    );

    let mut shared: Vec<(usize, usize)> = by_shape.into_iter().collect();
    shared.sort_by(|a, b| b.1.cmp(&a.1));
    println!("\nmost used shapes:");
    for (id, count) in shared.iter().take(6) {
        let canonical = byte(shapes[*id].get("canonical"));
        println!("  shape {id:>5} used by {count:>6} states, canonical {canonical}");
    }
}
