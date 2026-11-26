//! `hzr-export registries|command-registries|command-registries-full` - registry tables.

use std::path::Path;

#[path = "../../hzr-export/src/items_full.rs"]
mod items_full;

#[path = "../../hzr-export/src/registries.rs"]
mod registries;

/// Run one of this group's commands; `args` are the CLI arguments.
pub fn run(args: &[String]) -> Result<(), String> {
    match args {
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
            return Err(String::from("usage"));
        }
    }
}
