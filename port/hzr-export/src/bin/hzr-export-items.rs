//! `hzr-export items` - the per-item component table.

use std::path::Path;

#[path = "../items_codec.rs"]
mod items_codec;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [cmd, jar, out] if cmd == "items" => (|| -> Result<(), String> {
            let (map, _coverage) = items_codec::items_components(Path::new(jar))?;
            let mut object = serde_json::Map::new();
            for (key, value) in map {
                object.insert(key, value);
            }
            let text = serde_json::to_string_pretty(&serde_json::Value::Object(object))
                .map_err(|error| error.to_string())?;
            std::fs::write(out, format!("{text}\n")).map_err(|error| format!("{out}: {error}"))
        })(),
        _ => {
            eprintln!("hzr-export(items): usage: hzr-export-items items <client.jar> <out.json>");
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        eprintln!("hzr-export: {error}");
        std::process::exit(1);
    }
}
