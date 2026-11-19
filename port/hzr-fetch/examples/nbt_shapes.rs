//! Print the packed shapes of a table, to work out their encoding.
//!
//!   cargo run --offline --example nbt_shapes -- <file.nbt> [count]

use hzr_fetch::nbt_value::{self, Tag};
use std::path::Path;

fn longs(tag: Option<&Tag>) -> Vec<i64> {
    match tag {
        Some(Tag::LongArray(values)) => values.clone(),
        _ => Vec::new(),
    }
}

fn byte(tag: Option<&Tag>) -> i64 {
    match tag {
        Some(Tag::Byte(value)) => *value as i64,
        Some(Tag::Int(value)) => *value as i64,
        _ => 0,
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("an .nbt file");
    let count: usize = std::env::args()
        .nth(2)
        .and_then(|text| text.parse().ok())
        .unwrap_or(8);
    let bytes = std::fs::read(Path::new(&path)).unwrap();
    let (_, root) = nbt_value::parse(&bytes).expect("a table");

    let shapes = root
        .get("shapes")
        .and_then(|tag| match tag {
            Tag::List { items, .. } => Some(items.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let cached = match root.get("cached_shapes") {
        Some(Tag::IntArray(values)) => values.clone(),
        _ => Vec::new(),
    };
    let states = match root.get("states") {
        Some(Tag::List { items, .. }) => items.clone(),
        _ => Vec::new(),
    };

    println!("shapes {}, cached {}, states {}", shapes.len(), cached.len(), states.len());
    for (index, shape) in shapes.iter().take(count).enumerate() {
        let x = longs(shape.get("x"));
        let y = longs(shape.get("y"));
        let z = longs(shape.get("z"));
        println!(
            "\nshape {index}: cube {}, cube_axes {}, canonical {}, x {} longs, y {}, z {}",
            byte(shape.get("cube")),
            byte(shape.get("cube_axes")),
            byte(shape.get("canonical")),
            x.len(),
            y.len(),
            z.len()
        );
        for (axis, values) in [("x", &x), ("y", &y), ("z", &z)] {
            if values.is_empty() {
                continue;
            }
            let shown: Vec<String> = values
                .iter()
                .take(4)
                .map(|value| format!("{value:#018x}"))
                .collect();
            println!("   {axis}: {}", shown.join(" "));
        }
    }

    println!("\nfirst states:");
    for (index, state) in states.iter().take(count).enumerate() {
        let properties = match state {
            Tag::Compound(entries) => entries
                .iter()
                .map(|(key, value)| format!("{key}={}", short(value)))
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        };
        let target = cached.get(index).copied().unwrap_or(-1);
        println!("  {index:>6} -> shape {target:<6} {properties}");

    }
}

fn short(tag: &Tag) -> String {
    match tag {
        Tag::String(text) => text.clone(),
        Tag::Byte(value) => value.to_string(),
        Tag::Int(value) => value.to_string(),
        other => format!("{other:?}"),
    }
}
