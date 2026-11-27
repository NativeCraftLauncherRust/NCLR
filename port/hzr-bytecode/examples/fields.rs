use hzr_bytecode::classfile;
use std::path::Path;
fn main() {
    let path = std::env::args().nth(1).expect("a class file");
    let bytes = std::fs::read(Path::new(&path)).unwrap();
    let class = classfile::parse(&bytes).expect("a class file");
    let mut blocks = 0;
    let mut examples = Vec::new();
    for field in &class.fields {
        if field.descriptor == "Lnet/minecraft/world/level/block/Block;" {
            blocks += 1;
            if examples.len() < 3 { examples.push(field.name.clone()); }
        }
    }
    println!("{}: {} Block fields, {} fields total, examples {examples:?}", class.name, blocks, class.fields.len());
}
