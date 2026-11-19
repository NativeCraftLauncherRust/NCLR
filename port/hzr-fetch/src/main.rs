//! `hzr-fetch` command line: derive the resource tree from the player's own copy.

use hzr_fetch::{extract_jar, import_indexed, AssetIndex, Import, Receipt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const HELP: &str = "\
hzr-fetch: derive assets/, data/ and worldgen/ from the player's own Minecraft

Usage:
  hzr-fetch extract <CLIENT.jar> <OUT> [--index FILE.json] [--store DIR]
  hzr-fetch import  <OUT> --index FILE.json --store DIR [--sounds] [--fonts] [--languages] [--panorama]
  hzr-fetch inspect <VERSION.json>
  hzr-fetch model-states <FACTS.json> <OUT.json> [--compare PREPARED.json]

  extract   Copy every assets/, data/ and pack.png member of the jar under OUT.
            With --index and --store, a real indexed panorama face, font or
            language replaces the jar's placeholder.
  import    Write the indexed subtrees the flags name, verifying each object's
            SHA-1 and size against the index. Sounds are the 380 MB the jar
            never carries.
  inspect   Print the jar URL, the asset index and the Java a version JSON names.
";

fn main() -> ExitCode {
    match run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("hzr-fetch: error: {error}");
            ExitCode::FAILURE
        }
    }
}

struct Args {
    positional: Vec<String>,
    index: Option<PathBuf>,
    store: Option<PathBuf>,
    imports: Vec<Import>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut parsed = Args {
        positional: Vec::new(),
        index: None,
        store: None,
        imports: Vec::new(),
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--index" => parsed.index = Some(PathBuf::from(iter.next().ok_or("--index needs a file")?)),
            "--store" => parsed.store = Some(PathBuf::from(iter.next().ok_or("--store needs a folder")?)),
            "--sounds" => parsed.imports.push(Import::Sounds),
            "--fonts" => parsed.imports.push(Import::Fonts),
            "--languages" => parsed.imports.push(Import::Languages),
            "--panorama" => parsed.imports.push(Import::Panorama),
            "--packs" => parsed.imports.push(Import::Packs),
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
            _ => parsed.positional.push(arg.clone()),
        }
    }
    if parsed.imports.is_empty() {
        parsed.imports.extend_from_slice(&Import::ALL);
    }
    Ok(parsed)
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
            println!("hzr-fetch {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        "extract" => extract(&parse(&args[1..])?),
        "import" => import(&parse(&args[1..])?),
        "model-states" => model_states(&args[1..]),
        "block-order" => block_order(&args[1..]),
        "worldgen" => worldgen(&args[1..]),
        "gameplay" => gameplay(&args[1..]),
        "nbt" => nbt(&args[1..]),
        "bundle" => bundle(&args[1..]),
        "facts" => facts(&args[1..]),
        "inspect" => inspect(&args[1..]),
        other => Err(format!("unrecognized command {other}\n\n{HELP}")),
    }
}

fn load_index(path: &Path) -> Result<AssetIndex, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    AssetIndex::parse(&bytes)
}

fn report(label: &str, receipt: &Receipt) {
    println!(
        "{label}: {} files, {} bytes (jar {}, store {})",
        receipt.files, receipt.bytes, receipt.jar_files, receipt.store_files
    );
    if !receipt.fell_back_to_jar.is_empty() {
        println!(
            "  {} indexed files came from the jar (not in the asset store)",
            receipt.fell_back_to_jar.len()
        );
    }
    for skip in receipt.skipped.iter().take(10) {
        println!("  skipped {skip}");
    }
    if receipt.skipped.len() > 10 {
        println!("  ... and {} more", receipt.skipped.len() - 10);
    }
}

fn extract(args: &Args) -> Result<ExitCode, String> {
    let [jar, out] = args.positional.as_slice() else {
        return Err("extract takes <CLIENT.jar> <OUT>".into());
    };
    let index = match &args.index {
        Some(path) => Some(load_index(path)?),
        None => None,
    };
    let receipt = extract_jar(
        Path::new(jar),
        Path::new(out),
        index.as_ref(),
        args.store.as_deref(),
    )?;
    report("extracted", &receipt);
    match hzr_fetch::mirror_worldgen(Path::new(out)) {
        Ok(copied) => println!("worldgen: {copied} files mirrored from data/minecraft/worldgen"),
        Err(error) => println!("worldgen: skipped ({error})"),
    }
    Ok(ExitCode::SUCCESS)
}

fn import(args: &Args) -> Result<ExitCode, String> {
    let [out] = args.positional.as_slice() else {
        return Err("import takes <OUT>".into());
    };
    let index_path = args.index.as_ref().ok_or("import needs --index")?;
    let store = args.store.as_ref().ok_or("import needs --store")?;
    let index = load_index(index_path)?;
    let receipt = import_indexed(Path::new(out), &index, store, &args.imports)?;
    report("imported", &receipt);
    if !receipt.skipped.is_empty() {
        return Err(format!(
            "{} indexed files did not match the store; the game would run without them",
            receipt.skipped.len()
        ));
    }
    Ok(ExitCode::SUCCESS)
}

/// The first place two JSON values differ, as a path, for a readable failure.
fn first_difference(a: &serde_json::Value, b: &serde_json::Value, path: &str) -> Option<String> {
    use serde_json::Value;
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (key, value) in x {
                match y.get(key) {
                    None => return Some(format!("{path}.{key}: missing from the prepared file")),
                    Some(other) => {
                        if let Some(found) =
                            first_difference(value, other, &format!("{path}.{key}"))
                        {
                            return Some(found);
                        }
                    }
                }
            }
            for key in y.keys() {
                if !x.contains_key(key) {
                    return Some(format!("{path}.{key}: not produced by the derivation"));
                }
            }
            None
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Some(format!(
                    "{path}: {} elements, the prepared file has {}",
                    x.len(),
                    y.len()
                ));
            }
            for (index, (left, right)) in x.iter().zip(y.iter()).enumerate() {
                if let Some(found) = first_difference(left, right, &format!("{path}[{index}]")) {
                    return Some(found);
                }
            }
            None
        }
        _ if a == b => None,
        _ => Some(format!("{path}: {a} vs {b}")),
    }
}

/// `model-states.json` from `facts.json`, optionally comparing with a prepared one.
fn model_states(args: &[String]) -> Result<ExitCode, String> {
    let mut positional = Vec::new();
    let mut compare = None;
    let mut blockstates = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--compare" => {
                compare = Some(PathBuf::from(iter.next().ok_or("--compare needs a file")?))
            }
            "--blockstates" => {
                blockstates = Some(PathBuf::from(
                    iter.next().ok_or("--blockstates needs a folder")?,
                ))
            }
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
            _ => positional.push(arg.clone()),
        }
    }
    let [facts, out] = positional.as_slice() else {
        return Err("model-states takes <facts.json> <OUT.json>".into());
    };
    let bytes = std::fs::read(facts).map_err(|e| format!("{facts}: {e}"))?;
    let mut derived = hzr_fetch::derive_model_states(&bytes)?;
    let mut blocks = derived.as_object().map(|m| m.len()).unwrap_or(0);
    println!("derived {blocks} blocks from facts.json");

    if let Some(dir) = &blockstates {
        let mut known = derived.as_object().cloned().unwrap_or_default();
        let (added, unsupported) =
            hzr_fetch::model_states::synthesize_from_blockstates(dir, &known)?;
        for (name, entry) in &added {
            known.insert(name.clone(), entry.clone());
        }
        derived = serde_json::Value::Object(known);
        blocks = derived.as_object().map(|m| m.len()).unwrap_or(0);
        println!(
            "  + {} client-only blocks from blockstates ({blocks} total)",
            added.len()
        );
        for name in &unsupported {
            println!("  ! cannot rebuild {name} from its blockstate file");
        }
    }

    let text = serde_json::to_vec(&derived).map_err(|e| e.to_string())?;
    std::fs::write(out, &text).map_err(|e| format!("{out}: {e}"))?;
    println!("wrote {} bytes into {out}", text.len());

    if let Some(path) = compare {
        let real_bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let real: serde_json::Value =
            serde_json::from_slice(&real_bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        match first_difference(&derived, &real, "$") {
            None => println!("identical to {}", path.display()),
            Some(where_) => {
                println!("first difference: {where_}");
                return Ok(ExitCode::FAILURE);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `block-order.json`: the names of `facts.json`, in the order they appear.
fn block_order(args: &[String]) -> Result<ExitCode, String> {
    let [facts, out] = args else {
        return Err("block-order takes <facts.json> <OUT.json>".into());
    };
    let bytes = std::fs::read(facts).map_err(|e| format!("{facts}: {e}"))?;
    let order = hzr_fetch::facts::block_order(&bytes)?;
    let defaults = hzr_fetch::facts::defaults_from(&bytes)?;
    let document = serde_json::json!({ "blocks": order, "defaults": defaults });
    let text = serde_json::to_vec(&document).map_err(|e| e.to_string())?;
    std::fs::write(out, &text).map_err(|e| format!("{out}: {e}"))?;
    println!(
        "{} blocks, {} bytes into {out} (order + default state ids)",
        order.len(),
        text.len()
    );
    Ok(ExitCode::SUCCESS)
}

/// The world generator's bundles, from the jar's own data tree.
fn worldgen(args: &[String]) -> Result<ExitCode, String> {
    let mut positional = Vec::new();
    let mut compare = None;
    let mut facts = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--compare" => {
                compare = Some(PathBuf::from(iter.next().ok_or("--compare needs a folder")?))
            }
            "--facts" => facts = Some(PathBuf::from(iter.next().ok_or("--facts needs a file")?)),
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
            _ => positional.push(arg.clone()),
        }
    }
    let [data_root, out] = positional.as_slice() else {
        return Err("worldgen takes <DATA-ROOT> <OUT-DIR>".into());
    };
    let written = hzr_fetch::worldgen::write_all(Path::new(data_root), Path::new(out))?;
    let total: usize = written.iter().map(|(_, bytes)| bytes).sum();
    println!("{} bundles, {} bytes", written.len(), total);
    for (name, bytes) in &written {
        println!("  {name}.json {bytes} bytes");
    }

    if let Some(facts_path) = &facts {
        let bytes = std::fs::read(facts_path).map_err(|e| format!("{}: {e}", facts_path.display()))?;
        let flags = hzr_fetch::worldgen::block_flags(&bytes)?;
        let text = serde_json::to_vec(&flags).map_err(|e| e.to_string())?;
        let target = Path::new(out).join("worldgen").join("block_flags.json");
        std::fs::write(&target, &text).map_err(|e| format!("{}: {e}", target.display()))?;
        println!("  block_flags.json {} bytes", text.len());
    }

    if let Some(directory) = &compare {
        let mut failures = 0;
        let mut checked = 0;
        for (name, _) in hzr_fetch::worldgen::BUNDLES
            .iter()
            .map(|(name, _)| (*name, ()))
            .chain(std::iter::once(("block_flags", ())))
        {
            let prepared = directory.join(format!("{name}.json"));
            if !prepared.is_file() {
                println!("  {name}: no prepared file to compare");
                continue;
            }
            let produced_path = Path::new(out).join("worldgen").join(format!("{name}.json"));
            let produced: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&produced_path).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            let wanted: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&prepared).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            checked += 1;
            match first_difference(&produced, &wanted, &format!("${name}")) {
                None => println!("  {name}: identical"),
                Some(where_) => {
                    failures += 1;
                    println!("  {name}: {where_}");
                }
            }
        }
        println!("compared {checked} bundles, {failures} differ");
        if failures > 0 {
            return Ok(ExitCode::FAILURE);
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The `states` trailer and the `occlusion` copy of the block tables.
fn nbt(args: &[String]) -> Result<ExitCode, String> {
    let Some(mode) = args.first().map(String::as_str) else {
        return Err("nbt takes strip|restore|occlusion-strip|occlusion-restore ...".into());
    };
    match mode {
        "strip" => {
            let [_, input, body, blob] = args else {
                return Err("nbt strip takes <IN.nbt> <BODY.nbt> <BLOB.bin>".into());
            };
            let (body_len, blob_len) =
                hzr_fetch::nbt::strip_file(Path::new(input), Path::new(body), Path::new(blob))?;
            println!("stripped: {body_len} bytes of table, {blob_len} bytes of states trailer");
            Ok(ExitCode::SUCCESS)
        }
        "restore" => {
            let [_, body, blob, out] = args else {
                return Err("nbt restore takes <BODY.nbt> <BLOB.bin> <OUT.nbt>".into());
            };
            let total =
                hzr_fetch::nbt::restore_file(Path::new(body), Path::new(blob), Path::new(out))?;
            println!("restored: {total} bytes");
            Ok(ExitCode::SUCCESS)
        }
        "occlusion-strip" => {
            let [_, input, body, keyfile] = args else {
                return Err("nbt occlusion-strip takes <IN.nbt> <BODY.nbt> <KEY.bin>".into());
            };
            let bytes = std::fs::read(input).map_err(|e| format!("{input}: {e}"))?;
            let (stripped, span) = hzr_fetch::nbt::strip_key(&bytes, hzr_fetch::nbt::OCCLUSION_KEY)?;
            std::fs::write(body, &stripped).map_err(|e| format!("{body}: {e}"))?;
            std::fs::write(keyfile, hzr_fetch::nbt::encode_span(&span))
                .map_err(|e| format!("{keyfile}: {e}"))?;
            println!(
                "stripped occlusion at index {}: {} bytes of body, {} bytes removed",
                span.index,
                stripped.len(),
                span.bytes.len()
            );
            Ok(ExitCode::SUCCESS)
        }
        "occlusion-restore" => {
            let [_, body, keyfile, occlusion, out] = args else {
                return Err(
                    "nbt occlusion-restore takes <BODY.nbt> <KEY.bin> <OCCLUSION.nbt> <OUT.nbt>"
                        .into(),
                );
            };
            let body_bytes = std::fs::read(body).map_err(|e| format!("{body}: {e}"))?;
            let index_bytes = std::fs::read(keyfile).map_err(|e| format!("{keyfile}: {e}"))?;
            if index_bytes.len() != 4 {
                return Err(format!("{keyfile}: expected a 4-byte key index"));
            }
            let index = u32::from_le_bytes([index_bytes[0], index_bytes[1], index_bytes[2], index_bytes[3]]) as usize;
            let occlusion_bytes =
                std::fs::read(occlusion).map_err(|e| format!("{occlusion}: {e}"))?;
            let key = hzr_fetch::nbt::occlusion_key(&occlusion_bytes)?;
            let rebuilt = hzr_fetch::nbt::restore_key(
                &body_bytes,
                &hzr_fetch::nbt::KeySpan {
                    index,
                    bytes: key,
                },
            )?;
            std::fs::write(out, &rebuilt).map_err(|e| format!("{out}: {e}"))?;
            println!("restored occlusion: {} bytes", rebuilt.len());
            Ok(ExitCode::SUCCESS)
        }
        other => Err(format!("unknown nbt mode {other}")),
    }
}
/// The block of each state id, from the block order and the state table.
fn blocks_of_states(order: &Path, model_states: &Path) -> Result<Vec<String>, String> {
    let order_bytes = std::fs::read(order).map_err(|e| format!("{}: {e}", order.display()))?;
    let order_doc: serde_json::Value =
        serde_json::from_slice(&order_bytes).map_err(|e| format!("{}: {e}", order.display()))?;
    let names: Vec<String> = order_doc
        .get("blocks")
        .and_then(|value| value.as_array())
        .ok_or("block order has no blocks array")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "a block name is not a string".to_string())
        })
        .collect::<Result<_, _>>()?;
    let model_bytes =
        std::fs::read(model_states).map_err(|e| format!("{}: {e}", model_states.display()))?;
    let states = hzr_fetch::facts::from_model_states(&model_bytes)?;
    let mut out = Vec::new();
    for name in names {
        let count = states
            .get(&name)
            .ok_or_else(|| format!("{name}: no states in the table"))?
            .len();
        out.extend(std::iter::repeat_n(name, count));
    }
    Ok(out)
}

/// `gameplay-facts.json` re-encoded losslessly, or expanded back.
fn gameplay(args: &[String]) -> Result<ExitCode, String> {
    let mut positional = Vec::new();
    let mut compare = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--compare" => {
                compare = Some(PathBuf::from(iter.next().ok_or("--compare needs a file")?))
            }
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
            _ => positional.push(arg.clone()),
        }
    }
    let Some(mode) = positional.first().cloned() else {
        return Err("gameplay takes pack|expand ...".into());
    };
    match mode.as_str() {
        "pack" => {
            let [_, gameplay, model_states, order, out] = positional.as_slice() else {
                return Err(
                    "gameplay pack takes <GAMEPLAY.json> <MODEL-STATES.json> <ORDER.json> <OUT.json>"
                        .into(),
                );
            };
            let blocks = blocks_of_states(Path::new(order), Path::new(model_states))?;
            let bytes = std::fs::read(gameplay).map_err(|e| format!("{gameplay}: {e}"))?;
            let source = hzr_fetch::gameplay::read(&bytes, &blocks)?;
            let compact = hzr_fetch::gameplay::compress(&source)?;
            let text = serde_json::to_vec(&compact).map_err(|e| e.to_string())?;
            std::fs::write(out, &text).map_err(|e| format!("{out}: {e}"))?;
            println!(
                "{} states, {} blocks, {} bytes into {out}",
                blocks.len(),
                source.blocks.iter().collect::<std::collections::BTreeSet<_>>().len(),
                text.len()
            );
            let constant = compact["constant_fields"].as_array().map_or(0, Vec::len);
            println!(
                "  {} constant fields, {} varying fields",
                constant,
                compact["varying_fields"].as_array().map_or(0, Vec::len)
            );
            Ok(ExitCode::SUCCESS)
        }
        "expand" => {
            let [_, compact, out] = positional.as_slice() else {
                return Err("gameplay expand takes <COMPACT.json> <OUT.json>".into());
            };
            let bytes = std::fs::read(compact).map_err(|e| format!("{compact}: {e}"))?;
            let expanded = hzr_fetch::gameplay::expand(&bytes)?;
            let text = serde_json::to_vec(&expanded).map_err(|e| e.to_string())?;
            std::fs::write(out, &text).map_err(|e| format!("{out}: {e}"))?;
            let states = expanded["states"].as_array().map_or(0, Vec::len);
            println!("{states} states, {} bytes into {out}", text.len());
            if let Some(path) = compare {
                let original = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                let wanted: serde_json::Value = serde_json::from_slice(&original)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                match first_difference(&expanded, &wanted, "$") {
                    None => println!("identical to {}", path.display()),
                    Some(where_) => {
                        println!("first difference: {where_}");
                        return Ok(ExitCode::FAILURE);
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        other => Err(format!("unknown gameplay mode {other}")),
    }
}

/// `facts.json` rebuilt from `block-order.json` and `model-states.json`.
fn facts(args: &[String]) -> Result<ExitCode, String> {
    let mut positional = Vec::new();
    let mut compare = None;
    let mut defaults_from = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--compare" => {
                compare = Some(PathBuf::from(iter.next().ok_or("--compare needs a file")?))
            }
            "--defaults-from" => {
                defaults_from = Some(PathBuf::from(
                    iter.next().ok_or("--defaults-from needs a file")?,
                ))
            }
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
            _ => positional.push(arg.clone()),
        }
    }
    let [order, model_states, out] = positional.as_slice() else {
        return Err("facts takes <ORDER.json> <MODEL-STATES.json> <OUT.json>".into());
    };
    let order_bytes = std::fs::read(order).map_err(|e| format!("{order}: {e}"))?;
    let order_doc: serde_json::Value =
        serde_json::from_slice(&order_bytes).map_err(|e| format!("{order}: {e}"))?;
    let names: Vec<String> = order_doc
        .get("blocks")
        .and_then(|value| value.as_array())
        .ok_or("block order has no blocks array")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "a block name is not a string".to_string())
        })
        .collect::<Result<_, _>>()?;

    let model_bytes = std::fs::read(model_states).map_err(|e| format!("{model_states}: {e}"))?;
    let states = hzr_fetch::facts::from_model_states(&model_bytes)?;
    let defaults = match &defaults_from {
        Some(path) => {
            let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            Some(hzr_fetch::facts::defaults_from(&bytes)?)
        }
        // Without an explicit source, the order file's own defaults are used.
        None => order_doc
            .get("defaults")
            .and_then(|value| value.as_object())
            .map(|map| {
                map.iter()
                    .filter_map(|(name, value)| {
                        value.as_u64().map(|id| (name.clone(), id))
                    })
                    .collect::<std::collections::BTreeMap<String, u64>>()
            }),
    };
    let table = hzr_fetch::facts::Table {
        order: names,
        states,
        defaults,
    };
    let document = hzr_fetch::facts::build(&table)?;
    let text = serde_json::to_vec(&document).map_err(|e| e.to_string())?;
    std::fs::write(out, &text).map_err(|e| format!("{out}: {e}"))?;
    println!(
        "{} blocks, {} state ids, {} bytes, into {out}",
        table.order.len(),
        table.state_count(),
        text.len()
    );

    if let Some(path) = compare {
        let real_bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let real: serde_json::Value =
            serde_json::from_slice(&real_bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        // JSON object key order is not semantic; the comparison ignores it.
        match first_difference(&document, &real, "$") {
            None => println!("identical to {}", path.display()),
            Some(where_) => {
                println!("first difference: {where_}");
                return Ok(ExitCode::FAILURE);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn inspect(args: &[String]) -> Result<ExitCode, String> {    let [path] = args else {
        return Err("inspect takes <VERSION.json>".into());
    };
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let version = hzr_fetch::VersionJson::parse(&bytes)?;
    println!("id          {}", version.id);
    println!(
        "client      {} bytes, sha1 {}",
        version.downloads.client.size, version.downloads.client.sha1
    );
    println!("client url  {}", version.downloads.client.url);
    println!(
        "asset index {} ({} bytes, sha1 {})",
        version.asset_index.id, version.asset_index.size, version.asset_index.sha1
    );
    println!("index url   {}", version.asset_index.url);
    println!(
        "java        {}",
        version
            .java_major()
            .map(|major| major.to_string())
            .unwrap_or_else(|| "not stated".into())
    );
    Ok(ExitCode::SUCCESS)
}

/// Stage a prepared folder into what has to travel.
fn bundle(args: &[String]) -> Result<ExitCode, String> {
    let [resources, out] = args else {
        return Err("bundle takes <RESOURCES> <OUT-DIR>".into());
    };
    let report = hzr_fetch::bundle::bundle(Path::new(resources), Path::new(out))?;
    println!("staged {} files, {} bytes", report.files, report.bytes);
    println!(
        "  {} states trailers removed, one shared trailer of {} bytes",
        report.stripped_trailers, report.trailer
    );
    println!("  occlusion copy removed: {} bytes", report.stripped_occlusion);
    if report.dropped_duplicate {
        println!("  duplicate cached-collision.nbt dropped");
    }
    let total = report.bytes + report.trailer as u64;
    println!("  staged total: {total} bytes ({:.2} MiB)", total as f64 / (1u64 << 20) as f64);
    Ok(ExitCode::SUCCESS)
}