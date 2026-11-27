//! `hzr-export shapes` — the full shape bundle (collision/occlusion/support NBT).
//! Compiles the shape VM/families closure only.

use std::path::Path;

#[path = "../shape_scaffold.rs"]
mod shape_scaffold;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [cmd, jar, out] if cmd == "shapes" => {
            shape_scaffold::write(Path::new(jar), Path::new(out))
        }
        _ => {
            eprintln!("hzr-export(shapes): usage: hzr-export-shapes shapes <client.jar> <out-dir>");
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        eprintln!("hzr-export: {error}");
        std::process::exit(1);
    }
}
