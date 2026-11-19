//! `hzr-pack` command line: pack, verify, inspect and extract an `HZR3` archive.

use hzr_pack::{extract, pack, PackOptions, Reader};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const HELP: &str = "\
hzr-pack: one verifiable archive for prepared Horizon resources

Usage:
  hzr-pack pack <DIR> <OUT.hzr3> [--minecraft V] [--output-version N] [--schema N] [--include-droppable]
  hzr-pack verify <ARCHIVE.hzr3>
  hzr-pack info <ARCHIVE.hzr3>
  hzr-pack extract <ARCHIVE.hzr3> <DIR>

  pack      Pack DIR (a horizon-prepare output, e.g. resources/26.3) into OUT.
            Droppable files (logs, caches, the prepare stamp) are left out unless
            --include-droppable. Entries are sorted, so the archive is reproducible.
  verify    Re-hash every blob and check the archive against its manifest. Exit 1
            on a mismatch, a truncation, or a manifest that is not this format.
  info      Print the manifest: Minecraft version, preparer output version, schema,
            and one line per bucket with its file count and byte total.
  extract   Write every blob under DIR, refusing paths that climb out of it.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("hzr-pack: error: {error}");
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
        "-V" | "--version" => {
            println!("hzr-pack {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        "pack" => pack_command(&args[1..]),
        "verify" => verify_command(&args[1..]),
        "info" => info_command(&args[1..]),
        "extract" => extract_command(&args[1..]),
        other => Err(format!("unrecognized command {other}\n\n{HELP}")),
    }
}

/// `--name value` and `--flag` on a command line, leaving the positionals.
fn parse_options(args: &[String]) -> Result<(Vec<String>, PackOptions), String> {
    let mut positional = Vec::new();
    let mut options = PackOptions::default();
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--minecraft" => {
                options.minecraft = iter
                    .next()
                    .ok_or("--minecraft needs a value")?
                    .clone();
            }
            "--output-version" => {
                options.output_version = iter
                    .next()
                    .ok_or("--output-version needs a value")?
                    .parse()
                    .map_err(|_| "--output-version takes a number")?;
            }
            "--schema" => {
                options.schema = iter
                    .next()
                    .ok_or("--schema needs a value")?
                    .parse()
                    .map_err(|_| "--schema takes a number")?;
            }
            "--include-droppable" => options.include_droppable = true,
            "--only-packed" => options.only_packed = true,
            "--no-compress" => options.compress = false,
            "--omit" => {
                let path = iter.next().ok_or("--omit needs a path")?;
                options.omit.push(path.clone());
            }
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
            _ => positional.push(arg.clone()),
        }
    }
    Ok((positional, options))
}

fn pack_command(args: &[String]) -> Result<ExitCode, String> {
    let (positional, options) = parse_options(args)?;
    let [dir, out] = positional.as_slice() else {
        return Err("pack takes <DIR> <OUT.hzr3>".into());
    };
    let dir = Path::new(dir);
    let out = PathBuf::from(out);
    let manifest = pack(dir, &out, &options)?;
    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    let (computable, packed) = split(&manifest);
    println!(
        "packed {} files, {} bytes of content, into {} ({} bytes)",
        manifest.entries.len(),
        manifest.total_bytes,
        out.display(),
        size
    );
    println!("  must transfer   {packed} bytes");
    println!("  device-derivable {computable} bytes");
    if !manifest.omitted.is_empty() {
        println!("  omitted (regenerated on device): {}", manifest.omitted.join(", "));
    }
    for (letter, count) in bucket_counts(&manifest) {
        println!("  bucket {letter}: {count} files");
    }
    Ok(ExitCode::SUCCESS)
}

fn bucket_counts(manifest: &hzr_pack::Manifest) -> Vec<(&'static str, usize)> {
    let mut counts: std::collections::BTreeMap<&'static str, usize> = Default::default();
    for entry in &manifest.entries {
        *counts.entry(entry.bucket.letter()).or_insert(0) += 1;
    }
    counts.into_iter().collect()
}

/// Bytes the device derives itself, and bytes it must receive.
fn split(manifest: &hzr_pack::Manifest) -> (u64, u64) {
    let mut computable = 0;
    let mut packed = 0;
    for entry in &manifest.entries {
        // What travels is the stored form, which is deflated when that helps.
        let bytes = if entry.stored == 0 && !entry.compressed {
            entry.size
        } else {
            entry.stored
        };
        if entry.computable {
            computable += bytes;
        } else {
            packed += bytes;
        }
    }
    (computable, packed)
}

fn verify_command(args: &[String]) -> Result<ExitCode, String> {
    let [archive] = args else {
        return Err("verify takes <ARCHIVE.hzr3>".into());
    };
    let mut reader = Reader::open(Path::new(archive))?;
    let report = reader.verify()?;
    let manifest = reader.manifest();
    println!(
        "ok: {} files, {} bytes, Minecraft {}",
        report.files, report.bytes, manifest.minecraft
    );
    println!(
        "  must transfer   {} bytes",
        report.packed_bytes
    );
    println!(
        "  device-derivable {} bytes",
        report.computable_bytes
    );
    if report.missing_minecraft {
        eprintln!("hzr-pack: warning: the manifest does not name a Minecraft version");
    }
    Ok(ExitCode::SUCCESS)
}

fn info_command(args: &[String]) -> Result<ExitCode, String> {
    let [archive] = args else {
        return Err("info takes <ARCHIVE.hzr3>".into());
    };
    let reader = Reader::open(Path::new(archive))?;
    let manifest = reader.manifest();
    println!("format       {}", manifest.format);
    println!("minecraft    {}", manifest.minecraft);
    println!("preparer     output version {}", manifest.output_version);
    println!("schema       {}", manifest.schema);
    println!("created      {} (unix)", manifest.created_unix);
    println!("tables       {} bytes", manifest.total_bytes);
    println!("blob section {} bytes", manifest.blob_bytes);
    println!("blob base    {}", reader.blob_base());
    let (computable, packed) = split(manifest);
    println!("must transfer   {packed} bytes");
    println!("device-derivable {computable} bytes");
    let mut per_bucket: std::collections::BTreeMap<&'static str, (usize, u64)> = Default::default();
    for entry in &manifest.entries {
        let slot = per_bucket.entry(entry.bucket.letter()).or_insert((0, 0));
        slot.0 += 1;
        slot.1 += entry.size;
    }
    for (letter, (files, bytes)) in per_bucket {
        println!("bucket {letter}     {files} files, {bytes} bytes");
    }
    let unknown: Vec<&str> = manifest
        .entries
        .iter()
        .filter(|e| e.bucket == hzr_pack::Bucket::Unknown)
        .map(|e| e.path.as_str())
        .take(10)
        .collect();
    if !unknown.is_empty() {
        println!("unclassified (first 10 of the pack):");
        for path in unknown {
            println!("  {path}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn extract_command(args: &[String]) -> Result<ExitCode, String> {
    let [archive, dir] = args else {
        return Err("extract takes <ARCHIVE.hzr3> <DIR>".into());
    };
    let count = extract(Path::new(archive), Path::new(dir))?;
    println!("extracted {count} files into {}", Path::new(dir).display());
    Ok(ExitCode::SUCCESS)
}
