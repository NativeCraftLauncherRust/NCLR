//! Disassemble the start of a method, to see what a block's constructor looks like.
//!
//!   cargo run --offline --example dump_method -- <class file> <method> [instructions]

use hzr_bytecode::{classfile, disasm};
use std::path::Path;

fn main() {
    let path = std::env::args().nth(1).expect("a .class file");
    let method = std::env::args().nth(2).unwrap_or_else(|| "<clinit>".to_string());
    let limit: usize = std::env::args()
        .nth(3)
        .and_then(|text| text.parse().ok())
        .unwrap_or(60);
    let bytes = std::fs::read(Path::new(&path)).unwrap();
    let class = classfile::parse(&bytes).expect("a class file");
    println!("{} extends {} ({} methods)", class.name, class.super_name, class.methods.len());
    let Some(found) = class.method(&method) else {
        println!("no method {method}; names: {:?}", class.methods.iter().map(|m| m.name.clone()).collect::<Vec<_>>());
        return;
    };
    let Some(code) = &found.code else {
        println!("{method} has no code");
        return;
    };
    println!("{method}{} ({} bytes of code)", found.descriptor, code.bytes.len());
    for instruction in disasm::disassemble(&class, &code.bytes).iter().take(limit) {
        println!(
            "  {:>5}: {}",
            instruction.offset,
            disasm::describe(&class, instruction)
        );
    }
}
