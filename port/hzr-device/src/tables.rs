//! Generate `ship.hzr3`'s tables from the player's own `client.jar`.
//!
//! Every exporter command writes into the directory it is given, using the
//! same relative paths the shipped bundle had, so a single output root is all
//! the wiring needs. The commands are the ones `hzr-export` documents; the
//! group crates are the library form of the exporter's helper binaries.

use std::path::Path;

/// Commands that produce one of the 38 tables, in the order they run.
pub const COMMANDS: [&str; 28] = [
    "block-tags",
    "damage-tags",
    "damage-types",
    "damage-catalog",
    "player-float-ids",
    "entity-types",
    "attributes",
    "biome-parameters",
    "command-registries",
    "landing-definitions",
    "block-states",
    "registries",
    "shape-tables",
    "render-definitions",
    "datafix-tables",
    "entity-models",
    "worldgen-blocks",
    "model-states",
    "shapes",
    "structure-blocks",
    "commands",
    "items",
    "gameplay-compact",
    "block-behaviours",
    "creative-inventory",
    "mob-data",
    "occlusion-index",
    "prepare-stamp",
];

/// Run every table command with `out` as the resource root.
pub fn generate(client_jar: &Path, out: &Path) -> Result<(), String> {
    for command in COMMANDS {
        run(command, client_jar, out)?;
    }
    Ok(())
}

fn run(command: &str, client_jar: &Path, out: &Path) -> Result<(), String> {
    let args = vec![
        command.to_string(),
        client_jar.to_string_lossy().into_owned(),
        out.to_string_lossy().into_owned(),
    ];
    match command {
        "block-behaviours" => hzr_export_behaviour::run(&args),
        "landing-definitions" | "block-states" | "shape-tables" => {
            hzr_export_blocks::run(&args)
        }
        "shapes" => hzr_export_shapes::run(&args),
        "structure-blocks" => hzr_export_structure::run(&args),
        "registries" | "command-registries" => hzr_export_registries::run(&args),
        "commands" => hzr_export_commands::run(&args),
        "items" => hzr_export_items::run(&args),
        "gameplay-compact" => hzr_export_gameplay::run(&args),
        "creative-inventory" => hzr_export_creative::run(&args),
        "render-definitions" => hzr_export_render::run(&args),
        "mob-data" => hzr_export_mob::run(&args),
        "entity-types" | "attributes" => hzr_export_entity::run(&args),
        "model-states" | "entity-models" => hzr_export_models::run(&args),
        "biome-parameters" | "worldgen-blocks" | "datafix-tables" => {
            hzr_export_world::run(&args)
        }
        _ => match hzr_export::run_light(&args) {
            Some(result) => result,
            None => Err(format!("unknown exporter command {command}")),
        },
    }
}
