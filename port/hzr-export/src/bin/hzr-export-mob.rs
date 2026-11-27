//! `hzr-export mob-data` - mob placements/animations/state pathing table.

use std::path::Path;

#[path = "../../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;

#[path = "../mob_final.rs"]
mod mob_final;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [cmd, jar, out] if cmd == "mob-data" => {
            mob_final::write_mob_data(Path::new(jar), Path::new(out)).map(|_| ())
        }
        _ => {
            eprintln!("hzr-export(mob): usage: hzr-export-mob mob-data <client.jar> <out.json>");
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        eprintln!("hzr-export: {error}");
        std::process::exit(1);
    }
}
