//! `hzr-export render-definitions` - per-block render rule table.

use std::path::Path;

#[path = "../../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;

#[path = "../render_full.rs"]
mod render_full;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [cmd, jar, out] if cmd == "render-definitions" => {
            render_full::write_render_definitions(Path::new(jar), Path::new(out)).map(|_| ())
        }
        _ => {
            eprintln!("hzr-export(render): usage: hzr-export-render render-definitions <client.jar> <out.json>");
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        eprintln!("hzr-export: {error}");
        std::process::exit(1);
    }
}
