//! `hzr-export registries|command-registries|command-registries-full` - registry tables.

use std::path::Path;

#[path = "../items_full.rs"]
mod items_full;

#[path = "../registries.rs"]
mod registries;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [cmd, jar, out] if cmd == "registries" => {
            items_full::write(Path::new(jar), Path::new(out)).map(|_| ())
        }
        [cmd, jar, out] if cmd == "command-registries" => {
            registries::write(Path::new(jar), Path::new(out)).map(|_| ())
        }
        [cmd, jar, out] if cmd == "command-registries-full" => {
            items_full::write_command_registries(Path::new(jar), Path::new(out)).map(|_| ())
        }
        _ => {
            eprintln!("hzr-export(registries): usage: hzr-export-registries <registries|command-registries|command-registries-full> <client.jar> <out-dir>");
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        eprintln!("hzr-export: {error}");
        std::process::exit(1);
    }
}
