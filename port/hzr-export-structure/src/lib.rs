//! `hzr-export structure-blocks` — states/rotate/mirror/full_block per block state.

use std::path::Path;

#[path = "../../hzr-export/src/structure_final.rs"]
mod structure_final;

/// Run one of this group's commands; `args` are the CLI arguments.
pub fn run(args: &[String]) -> Result<(), String> {
    match args {
        [cmd, jar, out] if cmd == "structure-blocks" => {
            structure_final::write(Path::new(jar), Path::new(out)).map(|_| ())
        }
        _ => {
            eprintln!("hzr-export(structure): usage: hzr-export-structure structure-blocks <client.jar> <out-dir>");
            return Err(String::from("usage"));
        }
    }
}
