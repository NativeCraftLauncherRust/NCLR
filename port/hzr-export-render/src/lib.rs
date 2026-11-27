//! `hzr-export render-definitions` - per-block render rule table.

use std::path::Path;

#[path = "../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;

#[path = "../../hzr-export/src/render_full.rs"]
mod render_full;

/// Run one of this group's commands; `args` are the CLI arguments.
pub fn run(args: &[String]) -> Result<(), String> {
    match args {
        [cmd, jar, out] if cmd == "render-definitions" => {
            render_full::write_render_definitions(Path::new(jar), Path::new(out)).map(|_| ())
        }
        _ => {
            eprintln!("hzr-export(render): usage: hzr-export-render render-definitions <client.jar> <out.json>");
            return Err(String::from("usage"));
        }
    }
}
