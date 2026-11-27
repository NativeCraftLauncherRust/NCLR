//! `hzr-export commands` — the Brigadier command tree.

use std::path::Path;

#[path = "../commands_gamerule.rs"]
mod commands_gamerule;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [cmd, jar, out] if cmd == "commands" => {
            commands_gamerule::write(Path::new(jar), Path::new(out)).map(|_| ())
        }
        _ => {
            eprintln!("hzr-export(commands): usage: hzr-export-commands commands <client.jar> <out-dir>");
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        eprintln!("hzr-export: {error}");
        std::process::exit(1);
    }
}
