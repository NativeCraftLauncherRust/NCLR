use hzr_bytecode::{classfile, interp};
use std::collections::BTreeMap;
use std::path::Path;
fn main() {
    let path = std::env::args().nth(1).expect("a class file");
    let bytes = std::fs::read(Path::new(&path)).unwrap();
    let class = classfile::parse(&bytes).expect("a class file");
    let method = class.method("<clinit>").expect("<clinit>");
    let code = method.code.as_ref().expect("code");
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for instruction in interp::decode(&code.bytes) {
        if instruction.opcode != 0xb8 && instruction.opcode != 0xb6 && instruction.opcode != 0xb7 { continue; }
        if let Some((owner, name, _)) = classfile::method_ref(&class.constants, instruction.index.unwrap_or(0)) {
            let short = owner.rsplit('/').next().unwrap_or(&owner);
            *counts.entry(format!("{short}.{name}")).or_default() += 1;
        }
    }
    let mut sorted: Vec<(String, usize)> = counts.into_iter().collect();
    sorted.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    for (target, count) in sorted.iter() {
        println!("  {count:>5}  {target}");
    }
}
