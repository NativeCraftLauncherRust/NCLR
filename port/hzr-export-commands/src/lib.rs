//! `hzr-export commands` — the Brigadier command tree.

use std::path::Path;

#[path = "../../hzr-export/src/commands_gamerule.rs"]
mod commands_gamerule;

/// Run one of this group's commands; `args` are the CLI arguments.
pub fn run(args: &[String]) -> Result<(), String> {
    match args {
        [cmd, jar, out] if cmd == "commands" => {
            commands_gamerule::write(Path::new(jar), Path::new(out)).map(|_| ())
        }
        _ => {
            eprintln!("hzr-export(commands): usage: hzr-export-commands commands <client.jar> <out-dir>");
            return Err(String::from("usage"));
        }
    }
}
