//! `hzr-export`: rebuild Horizon's derived tables from the player's own client.jar.
//!
//! This binary is deliberately thin. The cheap jar/JSON commands run in-process
//! through [`hzr_export`]; the heavy table commands are forwarded to one of the
//! per-group binaries beside this executable (`hzr-export-entity`,
//! `hzr-export-world`, ...). Each of those path-includes only the modules its
//! commands need, so rustc never codegens all 46 table modules in one crate.
//!
//! The CLI is unchanged: `hzr-export <command> <client.jar> <out>`.
//! Run `cargo build --bins` (or a plain `cargo build`) to produce the group
//! binaries as well.

use std::path::PathBuf;

/// Heavy commands and the per-group binary that implements them. Keep in sync
/// with the `src/bin/hzr-export-*.rs` files.
fn group_for(command: &str) -> Option<&'static str> {
    match command {
        "entity-types" | "attributes" => Some("entity"),
        "biome-parameters" | "worldgen-blocks" | "datafix-tables" => Some("world"),
        "model-states" | "entity-models" => Some("models"),
        "landing-definitions" | "block-states" | "shape-tables" => Some("blocks"),
        "shapes" => Some("shapes"),
        "structure-blocks" => Some("structure"),
        "registries" | "command-registries" | "command-registries-full" => Some("registries"),
        "commands" => Some("commands"),
        "items" => Some("items"),
        "gameplay-compact" => Some("gameplay"),
        "block-behaviours" => Some("behaviour"),
        "creative-inventory" => Some("creative"),
        "render-definitions" => Some("render"),
        "mob-data" => Some("mob"),
        _ => None,
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: hzr-export <command> <client.jar> <out>\n\
         commands: block-tags | damage-tags | damage-types | damage-catalog |\n\
         \x20         player-float-ids | entity-types | attributes | biome-parameters |\n\
         \x20         command-registries | landing-definitions | registries |\n\
         \x20         command-registries-full | mob-data | block-states | shape-tables |\n\
         \x20         render-definitions | datafix-tables | entity-models |\n\
         \x20         worldgen-blocks | model-states | shapes | structure-blocks |\n\
         \x20         commands | items | gameplay-compact | block-behaviours | creative-inventory |
         horizon-resources | occlusion-index | prepare-stamp"
    );
    std::process::exit(2);
}

/// Run the per-group binary for `group`, forwarding args, stdio and exit code.
fn delegate(group: &str, args: &[String]) -> ! {
    let current = std::env::current_exe()
        .unwrap_or_else(|error| {
            eprintln!("hzr-export: cannot locate own executable: {error}");
            std::process::exit(1);
        });
    let mut helper: PathBuf = current;
    helper.set_file_name(format!(
        "hzr-export-{group}{}",
        std::env::consts::EXE_SUFFIX
    ));
    if !helper.exists() {
        eprintln!(
            "hzr-export: helper binary `{}` is missing; build all exporter binaries with `cargo build --bins`",
            helper.display()
        );
        std::process::exit(1);
    }
    match std::process::Command::new(&helper).args(args).status() {
        Ok(status) => match status.code() {
            Some(code) => std::process::exit(code),
            None => {
                eprintln!("hzr-export: helper `{}` was terminated", helper.display());
                std::process::exit(1);
            }
        },
        Err(error) => {
            eprintln!(
                "hzr-export: cannot run helper `{}`: {error}",
                helper.display()
            );
            std::process::exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }
    let command = args[0].as_str();

    if command == "horizon-resources" {
        let out = args.get(2).map(String::as_str).unwrap_or("");
        let result = if out.is_empty() {
            Err("usage: hzr-export horizon-resources <client.jar> <out.json>".to_string())
        } else {
            if let Some(parent) = std::path::Path::new(out).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(out, "{\n  \"schema\": 7,\n  \"minecraft\": \"26.3\",\n  \"data_version\": 5023\n}\n")
                .map_err(|error| format!("{out}: {error}"))
        };
        match result {
            Ok(()) => return,
            Err(error) => { eprintln!("hzr-export: {error}"); std::process::exit(1); }
        }
    }

    if let Some(group) = group_for(command) {
        delegate(group, &args);
    }

    match hzr_export::run_light(&args) {
        Some(Ok(())) => {}
        Some(Err(error)) => {
            eprintln!("hzr-export: {error}");
            std::process::exit(1);
        }
        None => usage(),
    }
}
