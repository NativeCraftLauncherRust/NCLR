//! `hzr-export biome-parameters|worldgen-blocks|datafix-tables` - jar-only world tables.

use std::path::Path;

#[path = "../../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;

#[path = "../worldgen.rs"]
mod worldgen;

#[path = "../worldgen_blocks.rs"]
mod worldgen_blocks;

#[path = "../datafix.rs"]
mod datafix;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [cmd, jar, out] if cmd == "biome-parameters" => {
            worldgen::export(jar, Path::new(out))
        }
        [cmd, jar, out] if cmd == "worldgen-blocks" => {
            worldgen_blocks::write(Path::new(jar), Path::new(out)).map(|_| ())
        }
        [cmd, jar, out] if cmd == "datafix-tables" => {
            datafix::write(Path::new(jar), Path::new(out)).map(|_| ())
        }
        _ => {
            eprintln!("hzr-export(world): usage: hzr-export-world <biome-parameters|worldgen-blocks|datafix-tables> <client.jar> <out>");
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        eprintln!("hzr-export: {error}");
        std::process::exit(1);
    }
}
