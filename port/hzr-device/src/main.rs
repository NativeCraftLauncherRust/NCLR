//! `hzr-device`: assemble a runnable resource tree from a shipped bundle.

use std::path::Path;
use std::process::ExitCode;

const HELP: &str = "\
hzr-device: assemble a runnable resource tree on the device

Usage:
  hzr-device assemble <ship.hzr3> <client.jar> <index.json> <objects/> <OUT> [--keep-shared]
  hzr-device check    <OUT>

  assemble     Unpack the bundle, rebuild its split tables, derive assets/data/
               worldgen from the player's own jar, then rebuild the tables the
               bundle left out. OUT must be empty or absent.
  --keep-shared  Leave the bundle's `_shared/` folder in place (debugging).
  check        Report which files a finished tree is missing.
";

fn main() -> ExitCode {
    match run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("hzr-device: error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    let Some(command) = args.first().map(String::as_str) else {
        print!("{HELP}");
        return Ok(ExitCode::from(2));
    };
    match command {
        "-h" | "--help" | "help" => {
            print!("{HELP}");
            Ok(ExitCode::SUCCESS)
        }
        "assemble" => {
            let mut keep_shared = false;
            let mut positional = Vec::new();
            for arg in args.iter().skip(1) {
                match arg.as_str() {
                    "--keep-shared" => keep_shared = true,
                    flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
                    _ => positional.push(arg.clone()),
                }
            }
            let [bundle, jar, index, store, out] = positional.as_slice() else {
                return Err("assemble takes <ship.hzr3> <client.jar> <index.json> <objects/> <OUT>".into());
            };
            let started = std::time::Instant::now();
            let done = if keep_shared {
                hzr_device::assemble(
                    Path::new(bundle),
                    Path::new(jar),
                    Path::new(index),
                    Path::new(store),
                    Path::new(out),
                )?
            } else {
                hzr_device::assemble_clean(
                    Path::new(bundle),
                    Path::new(jar),
                    Path::new(index),
                    Path::new(store),
                    Path::new(out),
                )?
            };
            println!(
                "assembled {} files in {:.1}s: {} unpacked, {} tables restored",
                done.unpacked + done.derived_files + done.imported_files,
                started.elapsed().as_secs_f32(),
                done.unpacked,
                done.restored_tables
            );
            println!(
                "  derived {} files, imported {}, {} worldgen bundles",
                done.derived_files, done.imported_files, done.worldgen_bundles
            );
            println!("  {} bytes written into {out}", done.bytes);
            let missing = hzr_device::missing_from(Path::new(out));
            if !missing.is_empty() {
                eprintln!("  still missing: {missing:?}");
                return Ok(ExitCode::FAILURE);
            }
            Ok(ExitCode::SUCCESS)
        }
        "clean" => {
            let [jar_dir, store] = args[1..].get(0..2).unwrap_or_default() else {
                return Err("clean takes <JAR_DIR> <ASSETS_DIR>".into());
            };
            let freed = hzr_device::forget_downloads(Path::new(jar_dir), Path::new(store))?;
            println!("freed {freed} bytes");
            Ok(ExitCode::SUCCESS)
        }
        "check" => {
            let [out] = args[1..].as_ref() else {
                return Err("check takes <OUT>".into());
            };
            let missing = hzr_device::missing_from(Path::new(out));
            if missing.is_empty() {
                println!("ok");
                Ok(ExitCode::SUCCESS)
            } else {
                for name in missing {
                    println!("{}", name.display());
                }
                Ok(ExitCode::FAILURE)
            }
        }
        other => Err(format!("unrecognized command {other}\n\n{HELP}")),
    }
}
