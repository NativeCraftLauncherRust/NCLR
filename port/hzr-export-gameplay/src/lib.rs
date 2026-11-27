//! `hzr-export gameplay-compact` - the shared gameplay facts table.

use std::path::Path;

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

#[path = "../../hzr-export/src/gameplay_document.rs"]
mod gameplay_document;

/// Run one of this group's commands; `args` are the CLI arguments.
pub fn run(args: &[String]) -> Result<(), String> {
    match args {
        [cmd, jar, out] if cmd == "gameplay-compact" => (|| -> Result<(), String> {
            let value = gameplay_document::document(Path::new(jar))?;
            let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
            std::fs::write(out, format!("{text}\n")).map_err(|e| format!("{out}: {e}"))
        })(),
        _ => { eprintln!("hzr-export(gameplay): usage: hzr-export-gameplay gameplay-compact <client.jar> <out.json>"); return Err(String::from("usage")); }
    }
}
