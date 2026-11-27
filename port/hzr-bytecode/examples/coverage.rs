use hzr_bytecode::{classfile, interp};
use std::path::Path;
fn main() {
    let path = std::env::args().nth(1).expect("a class file");
    let bytes = std::fs::read(Path::new(&path)).unwrap();
    let class = classfile::parse(&bytes).expect("a class file");
    let method = class.method("<clinit>").expect("<clinit>");
    let code = method.code.as_ref().expect("code");
    let decoded = interp::decode(&code.bytes);
    let consumed: usize = decoded.iter().map(|i| i.length).sum();
    println!(
        "code {} bytes, {} instructions, consumed {consumed} ({} left)",
        code.bytes.len(),
        decoded.len(),
        code.bytes.len() as i64 - consumed as i64
    );
}
