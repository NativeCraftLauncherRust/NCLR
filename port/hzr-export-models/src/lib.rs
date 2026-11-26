//! `hzr-export model-states|entity-models` - model/state tables.

use std::path::Path;

#[path = "../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;

#[path = "../../hzr-export/src/model_states.rs"]
mod model_states;

#[path = "../../hzr-export/src/models.rs"]
mod models;

/// Run one of this group's commands; `args` are the CLI arguments.
pub fn run(args: &[String]) -> Result<(), String> {
    match args {
        [cmd, jar, out] if cmd == "model-states" => {
            model_states::write(Path::new(jar), Path::new(out)).map(|_| ())
        }
        [cmd, jar, out] if cmd == "entity-models" => {
            models::write_entity_models(Path::new(jar), out).map(|_| ())
        }
        _ => {
            eprintln!("hzr-export(models): usage: hzr-export-models <model-states|entity-models> <client.jar> <out>");
            return Err(String::from("usage"));
        }
    }
}
