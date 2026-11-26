//! `hzr-export creative-inventory` - the creative inventory table.

use std::path::Path;

#[path = "../../hzr-export/src/creative_search.rs"]
mod creative_search;

/// Run one of this group's commands; `args` are the CLI arguments.
pub fn run(args: &[String]) -> Result<(), String> {
    match args {
        [cmd, jar, out] if cmd == "creative-inventory" => {
            creative_search::write(Path::new(jar), Path::new(out)).map(|_| ())
        }
        _ => { eprintln!("hzr-export(creative): usage: hzr-export-creative creative-inventory <client.jar> <out-dir>"); return Err(String::from("usage")); }
    }
}
