//! `hzr-export landing-definitions|block-states|shape-tables` - block tables
//! that need the state/shape registries but not the full shape VM.

use std::path::Path;

#[path = "../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;

#[path = "../../hzr-export/src/block.rs"]
mod block;

#[path = "../../hzr-export/src/states.rs"]
mod states;

#[path = "../../hzr-export/src/shape.rs"]
mod shape;

/// Run one of this group's commands; `args` are the CLI arguments.
pub fn run(args: &[String]) -> Result<(), String> {
    match args {
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
            return Err(String::from("usage"));
        }
    }
}
