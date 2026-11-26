//! `hzr-export biome-parameters|worldgen-blocks|datafix-tables` - jar-only world tables.

use std::path::Path;

#[path = "../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;

#[path = "../../hzr-export/src/worldgen.rs"]
mod worldgen;

#[path = "../../hzr-export/src/worldgen_blocks.rs"]
mod worldgen_blocks;

#[path = "../../hzr-export/src/datafix.rs"]
mod datafix;

/// Run one of this group's commands; `args` are the CLI arguments.
pub fn run(args: &[String]) -> Result<(), String> {
    match args {
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
            return Err(String::from("usage"));
        }
    }
}
