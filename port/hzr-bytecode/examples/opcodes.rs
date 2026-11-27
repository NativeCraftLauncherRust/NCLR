//! Opcode histogram for a method, to see what the evaluator must support.
//!
//!   cargo run --offline --example opcodes -- <class file> <method>

use hzr_bytecode::classfile;
use std::collections::BTreeMap;
use std::path::Path;

fn main() {
    let path = std::env::args().nth(1).expect("a class file");
    let method_name = std::env::args().nth(2).unwrap_or_else(|| "<clinit>".into());
    let bytes = std::fs::read(Path::new(&path)).unwrap();
    let class = classfile::parse(&bytes).expect("a class file");
    let Some(method) = class.method(&method_name) else {
        println!("no method {method_name}");
        return;
    };
    let Some(code) = &method.code else {
        println!("{method_name} has no code");
        return;
    };
    let mut counts: BTreeMap<u8, usize> = BTreeMap::new();
    for instruction in hzr_bytecode::interp::decode(&code.bytes) {
        *counts.entry(instruction.opcode).or_default() += 1;
    }
    let mut sorted: Vec<(u8, usize)> = counts.into_iter().collect();
    sorted.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    println!(
        "{method_name}: {} bytes, {} decoded instructions, {} distinct opcodes",
        code.bytes.len(),
        sorted.iter().map(|(_, c)| c).sum::<usize>(),
        sorted.len()
    );
    for (opcode, count) in sorted.iter().take(24) {
        println!("  {opcode:#04x} x{count}");
    }
}
