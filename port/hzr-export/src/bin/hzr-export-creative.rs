//! `hzr-export creative-inventory` - the creative inventory table.

use std::path::Path;

#[path = "../creative_search.rs"]
mod creative_search;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [cmd, jar, out] if cmd == "creative-inventory" => {
            creative_search::write(Path::new(jar), Path::new(out)).map(|_| ())
        }
        _ => { eprintln!("hzr-export(creative): usage: hzr-export-creative creative-inventory <client.jar> <out-dir>"); std::process::exit(2); }
    };
    if let Err(e) = result { eprintln!("hzr-export: {e}"); std::process::exit(1); }
}
