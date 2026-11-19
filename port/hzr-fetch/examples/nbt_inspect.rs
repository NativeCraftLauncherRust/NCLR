//! Describe a block table's contents: list lengths and the shape of one entry.
//!
//!   cargo run --offline --example nbt_inspect -- <file.nbt>

use hzr_fetch::nbt_value::{self, Tag};
use std::path::Path;

fn describe(name: &str, tag: &Tag) {
    match tag {
        Tag::List { element, items } => {
            println!("  {name:<16} list of tag {element}, {} items", items.len());
            if let Some(first) = items.first() {
                match first {
                    Tag::Compound(entries) => {
                        let keys: Vec<String> = entries
                            .iter()
                            .take(6)
                            .map(|(key, value)| {
                                format!("{key}:{}", short(value))
                            })
                            .collect();
                        println!("      first: {{{}}}", keys.join(", "));
                    }
                    other => println!("      first: {}", short(other)),
                }
            }
        }
        Tag::Compound(entries) => {
            println!("  {name:<16} compound, {} keys", entries.len());
            for (key, value) in entries.iter().take(3) {
                println!("      {key}: {}", short(value));
            }
        }
        Tag::LongArray(values) => {
            println!("  {name:<16} long array, {} longs", values.len());
            if let Some(first) = values.first() {
                println!("      first bits set: {:?}", bits(*first));
            }
        }
        other => println!("  {name:<16} {}", short(other)),
    }
}

fn bits(value: i64) -> Vec<u32> {
    let mut set = Vec::new();
    for bit in 0..64 {
        if value & (1i64 << bit) != 0 {
            set.push(bit);
        }
    }
    set
}

fn short(tag: &Tag) -> String {
    match tag {
        Tag::Byte(v) => format!("byte {v}"),
        Tag::Short(v) => format!("short {v}"),
        Tag::Int(v) => format!("int {v}"),
        Tag::Long(v) => format!("long {v}"),
        Tag::Float(v) => format!("float {v}"),
        Tag::Double(v) => format!("double {v}"),
        Tag::ByteArray(v) => format!("byte[{}]", v.len()),
        Tag::String(v) => format!("string {v:?}"),
        Tag::List { element, items } => format!("list<{element}>[{}]", items.len()),
        Tag::Compound(v) => format!("compound[{}]", v.len()),
        Tag::IntArray(v) => format!("int[{}]", v.len()),
        Tag::LongArray(v) => format!("long[{}]", v.len()),
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("an .nbt file");
    let path = Path::new(&path);
    let bytes = std::fs::read(path).unwrap();
    let (root_name, tag) = nbt_value::parse(&bytes).expect("a parseable table");
    println!("{path:?}: {} bytes, root {root_name:?}", bytes.len());
    for (key, value) in tag.as_compound().unwrap() {
        describe(key, value);
    }
}
