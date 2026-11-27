//! `hzr-export shapes` — the full shape bundle (collision/occlusion/support NBT).
//! Compiles the shape VM/families closure only.

use std::path::Path;

#[path = "../../hzr-export/src/shape_scaffold.rs"]
mod shape_scaffold;

/// Run one of this group's commands; `args` are the CLI arguments.
pub fn run(args: &[String]) -> Result<(), String> {
    match args {
        [cmd, jar, out] if cmd == "shapes" => {
            shape_scaffold::write(Path::new(jar), Path::new(out))
        }
        _ => {
            eprintln!("hzr-export(shapes): usage: hzr-export-shapes shapes <client.jar> <out-dir>");
            return Err(String::from("usage"));
        }
    }
}
