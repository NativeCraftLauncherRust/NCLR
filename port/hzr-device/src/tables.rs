//! Generate `ship.hzr3`'s tables from the player's own `client.jar`.
//!
//! Every exporter command writes into the directory it is given, using the
//! same relative paths the shipped bundle had, so a single output root is all
//! the wiring needs. The commands are the ones `hzr-export` documents; the
//! group crates are the library form of the exporter's helper binaries.

use std::path::{Path, PathBuf};

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
    "command-registries-full",
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
    generate_with_progress(client_jar, out, &|_, _| {})
}

/// Same, reporting `(done, total)` before each command runs.
pub fn generate_with_progress(
    client_jar: &Path,
    out: &Path,
    progress: &dyn Fn(usize, usize),
) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    for (index, command) in COMMANDS.iter().enumerate() {
        progress(index, COMMANDS.len());
        let target = target(command, out);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        run(command, client_jar, &target)?;
    }
    progress(COMMANDS.len(), COMMANDS.len());
    Ok(())
}

/// File-writing commands take their own path; the rest take the resource root
/// and write their fixed relative names into it.
fn target(command: &str, out: &Path) -> PathBuf {
    let file = match command {
        "block-tags" => Some("block-tags.json"),
        "damage-tags" => Some("damage-tags.json"),
        "damage-types" => Some("damage-types.json"),
        "damage-catalog" => Some("damage-catalog.json"),
        "player-float-ids" => Some("player-float-ids.json"),
        "entity-types" => Some("entity-types.json"),
        "attributes" => Some("attributes.json"),
        "render-definitions" => Some("render-definitions.json"),
        "entity-models" => Some("entity-models.json"),
        "items" => Some("items.json"),
        "gameplay-compact" => Some("_shared/gameplay.compact.json"),
        "block-behaviours" => Some("block-behaviours.json"),
        "creative-inventory" => Some("creative-inventory.json"),
        "mob-data" => Some("mob-data.json"),
        "occlusion-index" => Some("_shared/occlusion.index"),
        "prepare-stamp" => Some("horizon-prepare.json"),
        _ => None,
    };
    match file {
        Some(rel) => out.join(rel),
        None => out.to_path_buf(),
    }
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
        "registries" | "command-registries" | "command-registries-full" => {
            hzr_export_registries::run(&args)
        }
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
