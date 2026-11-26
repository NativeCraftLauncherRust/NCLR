//! `hzr-export gameplay-compact` - the shared gameplay facts table.

use std::path::Path;

#[path = "../../../hzr-bytecode/src/classfile.rs"]
mod classfile;

#[path = "../gameplay_document.rs"]
mod gameplay_document;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [cmd, jar, out] if cmd == "gameplay-compact" => (|| -> Result<(), String> {
            let value = gameplay_document::document(Path::new(jar))?;
            let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
            std::fs::write(out, format!("{text}\n")).map_err(|e| format!("{out}: {e}"))
        })(),
        _ => { eprintln!("hzr-export(gameplay): usage: hzr-export-gameplay gameplay-compact <client.jar> <out.json>"); std::process::exit(2); }
    };
    if let Err(e) = result { eprintln!("hzr-export: {e}"); std::process::exit(1); }
}
