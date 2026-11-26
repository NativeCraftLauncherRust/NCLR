//! `hzr-export block-behaviours` - the block behaviour table.

use std::path::Path;

#[path = "../../../hzr-bytecode/src/classfile.rs"]
mod classfile;

#[path = "../behaviour_complete2.rs"]
mod behaviour_complete2;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [cmd, jar, out] if cmd == "block-behaviours" => (|| -> Result<(), String> {
            let (value, _cov) = behaviour_complete2::document(Path::new(jar))?;
            let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
            std::fs::write(out, format!("{text}\n")).map_err(|e| format!("{out}: {e}"))
        })(),
        _ => { eprintln!("hzr-export(behaviour): usage: hzr-export-behaviour block-behaviours <client.jar> <out.json>"); std::process::exit(2); }
    };
    if let Err(e) = result { eprintln!("hzr-export: {e}"); std::process::exit(1); }
}
