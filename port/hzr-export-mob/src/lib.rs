//! `hzr-export mob-data` - mob placements/animations/state pathing table.

use std::path::Path;

#[path = "../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;

#[path = "../../hzr-export/src/mob_final.rs"]
mod mob_final;

/// Run one of this group's commands; `args` are the CLI arguments.
pub fn run(args: &[String]) -> Result<(), String> {
    match args {
        [cmd, jar, out] if cmd == "mob-data" => {
            mob_final::write_mob_data(Path::new(jar), Path::new(out)).map(|_| ())
        }
        _ => {
            eprintln!("hzr-export(mob): usage: hzr-export-mob mob-data <client.jar> <out.json>");
            return Err(String::from("usage"));
        }
    }
}
