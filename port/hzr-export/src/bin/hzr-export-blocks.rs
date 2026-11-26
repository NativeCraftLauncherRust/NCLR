//! `hzr-export landing-definitions|block-states|shape-tables` - block tables
//! that need the state/shape registries but not the full shape VM.

use std::path::Path;

#[path = "../../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;

#[path = "../block.rs"]
mod block;

#[path = "../states.rs"]
mod states;

#[path = "../shape.rs"]
mod shape;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [cmd, jar, out] if cmd == "landing-definitions" => {
            block::write(Path::new(jar), Path::new(out)).map(|_| ())
        }
        [cmd, jar, out] if cmd == "block-states" => {
            states::write(Path::new(jar), Path::new(out)).map(|_| ())
        }
        [cmd, jar, out] if cmd == "shape-tables" => {
            shape::write(Path::new(jar), Path::new(out)).map(|_| ())
        }
        _ => {
            eprintln!("hzr-export(blocks): usage: hzr-export-blocks <landing-definitions|block-states|shape-tables> <client.jar> <out>");
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        eprintln!("hzr-export: {error}");
        std::process::exit(1);
    }
}
