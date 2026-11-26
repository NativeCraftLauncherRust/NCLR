use hzr_bytecode::classfile;
use std::path::Path;
fn main() {
    let path = std::env::args().nth(1).expect("a class file");
    let bytes = std::fs::read(Path::new(&path)).unwrap();
    let class = classfile::parse(&bytes).expect("a class file");
    for method in &class.methods {
        if method.descriptor.contains("DyeColor") {
            println!("  {} {}", method.name, method.descriptor);
        }
    }
}
