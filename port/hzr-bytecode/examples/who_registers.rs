//! Which methods of a class register blocks, and how often.
//!
//!   cargo run --offline --example who_registers -- <Blocks.class>

use hzr_bytecode::classfile;
use std::path::Path;

fn main() {
    let path = std::env::args().nth(1).expect("Blocks.class");
    let bytes = std::fs::read(Path::new(&path)).unwrap();
    let class = classfile::parse(&bytes).expect("a class file");
    println!("{}: {} methods", class.name, class.methods.len());
    let mut total = 0;
    for method in &class.methods {
        let Some(code) = &method.code else { continue };
        let mut registers = 0;
        let mut puts = 0;
        for instruction in hzr_bytecode::interp::decode(&code.bytes) {
            if instruction.opcode != 0xb8 && instruction.opcode != 0xb3 {
                continue;
            }
            let Some((owner, name, descriptor)) =
                classfile::method_ref(&class.constants, instruction.index.unwrap_or(0))
            else {
                if instruction.opcode == 0xb3 {
                    if let Some((owner, _, descriptor)) =
                        classfile::field_ref(&class.constants, instruction.index.unwrap_or(0))
                    {
                        if owner.ends_with("Blocks") && descriptor.starts_with("Lnet/minecraft/world/level/block/Block") {
                            puts += 1;
                        }
                    }
                }
                continue;
            };
            if owner.ends_with("Blocks") && name == "register" {
                registers += 1;
            }
            let _ = descriptor;
        }
        if registers > 0 || puts > 0 {
            total += registers;
            println!(
                "  {:<24}{}  register x{}, putstatic x{}",
                method.name, method.descriptor, registers, puts
            );
        }
    }
    println!("register calls outside <clinit>: {total}");
}
