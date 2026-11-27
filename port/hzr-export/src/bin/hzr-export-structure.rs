//! `hzr-export structure-blocks` — states/rotate/mirror/full_block per block state.

use std::path::Path;

#[path = "../structure_final.rs"]
mod structure_final;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [cmd, jar, out] if cmd == "structure-blocks" => {
            structure_final::write(Path::new(jar), Path::new(out)).map(|_| ())
        }
        _ => {
            eprintln!("hzr-export(structure): usage: hzr-export-structure structure-blocks <client.jar> <out-dir>");
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        eprintln!("hzr-export: {error}");
        std::process::exit(1);
    }
}
