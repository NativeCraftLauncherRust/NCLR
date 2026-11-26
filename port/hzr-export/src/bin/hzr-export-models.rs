//! `hzr-export model-states|entity-models` - model/state tables.

use std::path::Path;

#[path = "../../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;

#[path = "../model_states.rs"]
mod model_states;

#[path = "../models.rs"]
mod models;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [cmd, jar, out] if cmd == "model-states" => {
            model_states::write(Path::new(jar), Path::new(out)).map(|_| ())
        }
        [cmd, jar, out] if cmd == "entity-models" => {
            models::write_entity_models(Path::new(jar), out).map(|_| ())
        }
        _ => {
            eprintln!("hzr-export(models): usage: hzr-export-models <model-states|entity-models> <client.jar> <out>");
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        eprintln!("hzr-export: {error}");
        std::process::exit(1);
    }
}
