//! Print the first few per-block entries of a table's `blocks` compound.
//!
//!   cargo run --offline --example nbt_blocks -- <file.nbt> [count]

use hzr_fetch::nbt_value::{self, Tag};
use std::path::Path;

fn main() {
    let path = std::env::args().nth(1).expect("an .nbt file");
    let count: usize = std::env::args()
        .nth(2)
        .and_then(|text| text.parse().ok())
        .unwrap_or(3);
    let bytes = std::fs::read(Path::new(&path)).unwrap();
    let (_, root) = nbt_value::parse(&bytes).expect("a table");
    let Some(blocks) = root.get("blocks").and_then(|tag| tag.as_compound()) else {
        println!("no blocks compound");
        return;
    };
    println!("{path}: {} blocks", blocks.len());
    for (name, value) in blocks.iter().take(count) {
        println!("  {name} -> {value:?}");
    }
    // How many distinct value shapes?
    let mut kinds: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for (_, value) in blocks {
        *kinds.entry(format!("{:?}", value.id())).or_default() += 1;
    }
    println!("  tags: {kinds:?}");
}
