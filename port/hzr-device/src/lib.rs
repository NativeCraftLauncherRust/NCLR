//! Assembles a runnable resource tree on the device.
//!
//! One function, because this is what the iOS shell calls: unpack the shipped
//! bundle, put its split tables back together, derive `assets`, `data` and
//! `worldgen` from the player's own jar, then rebuild the tables that were left
//! out. After it returns, the folder is what `horizon-client --resources` wants.
//!
//! The order matters, and this file only fixes the order; every piece carries
//! its own tests:
//!
//! ```text
//! ship.hzr3  -> extract -> split tables and shared blobs
//!                          |
//!                          +-- states.bin     -> four tables' `states` trailers
//!                          +-- occlusion.key  -> block-properties.nbt
//!   occlusion.nbt       -> block-properties.nbt (the embedded copy)
//!   cached-collision    -> worldgen/blocks/cached-collision.nbt (duplicate)
//!
//! client.jar + asset store -> assets/, data/, worldgen/<trees>
//! shared inputs            -> facts.json, model-states.json,
//!                             gameplay-facts.json, worldgen/*.json
//! ```
//!
//! `assemble_clean` stages the work in a sibling folder, verifies it, and only
//! then swaps it into place, so a failed run never costs the player a working
//! tree.

pub mod tables;

use hzr_fetch::{bundle as staging, facts, gameplay, jar, model_states, nbt, worldgen};
use std::fs;
use std::path::{Path, PathBuf};

/// What assembling did, for the shell's progress line.
#[derive(Debug, Default, Clone)]
pub struct Assembled {
    pub unpacked: usize,
    pub restored_tables: usize,
    pub derived_files: usize,
    pub imported_files: usize,
    pub worldgen_bundles: usize,
    pub bytes: u64,
}

/// Turn an I/O error into something a person can act on.
///
/// Assembling writes about 250 MB; on a phone the disk filling up is a real
/// outcome, and "No space left on device (os error 28)" alone does not tell the
/// player what to do.
pub fn explain(error: &std::io::Error) -> String {
    match error.raw_os_error() {
        // ENOSPC on Unix, ERROR_DISK_FULL / ERROR_HANDLE_DISK_FULL on Windows.
        Some(28) | Some(112) | Some(39) => format!("{error}: the device has no space left"),
        // EACCES on Unix, ERROR_ACCESS_DENIED on Windows.
        Some(13) | Some(5) => format!("{error}: this app may not write there"),
        _ => format!("{error}"),
    }
}
fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("{}: {}", path.display(), explain(&e)))
}

fn write(path: &Path, bytes: &[u8]) -> Result<u64, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {}", parent.display(), explain(&e)))?;
    }
    fs::write(path, bytes).map_err(|e| format!("{}: {}", path.display(), explain(&e)))?;
    Ok(bytes.len() as u64)
}

/// What a verification found, for the shell's log line.
#[derive(Debug, Default, Clone)]
pub struct Verified {
    pub blocks: usize,
    pub states: usize,
    pub tables: usize,
    pub assets: usize,
    pub worldgen: usize,
}

fn count_files(root: &Path) -> usize {
    let mut total = 0;
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total += count_files(&path);
        } else {
            total += 1;
        }
    }
    total
}

/// The first `count` bytes, for a shape check that does not parse.
fn head_bytes(path: &Path, count: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut file = fs::File::open(path).map_err(|e| format!("{}: {}", path.display(), explain(&e)))?;
    let mut buffer = vec![0u8; count];
    let read = file
        .read(&mut buffer)
        .map_err(|e| format!("{}: {}", path.display(), explain(&e)))?;
    buffer.truncate(read);
    Ok(buffer)
}

/// Read the two ends of a file without parsing it.
fn ends(path: &Path) -> Result<(u8, u8), String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(path).map_err(|e| format!("{}: {}", path.display(), explain(&e)))?;
    let mut first = [0u8; 1];
    file.read_exact(&mut first)
        .map_err(|e| format!("{}: {}", path.display(), explain(&e)))?;
    let length = file
        .metadata()
        .map_err(|e| format!("{}: {}", path.display(), explain(&e)))?
        .len();
    if length < 2 {
        return Err(format!("{}: {length} bytes", path.display()));
    }
    file.seek(SeekFrom::End(-1))
        .map_err(|e| format!("{}: {}", path.display(), explain(&e)))?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last)
        .map_err(|e| format!("{}: {}", path.display(), explain(&e)))?;
    Ok((first[0], last[0]))
}

/// Is the assembled tree actually usable?
///
/// Presence is not enough: a table can exist and be truncated, or parse and be
/// empty. This reads what the engine reads, so a broken assembly fails here
/// rather than inside the loader with no explanation.
pub fn verify(out: &Path) -> Result<Verified, String> {
    let mut counts = Verified::default();

    let facts_bytes = read(&out.join("facts.json"))?;
    let blocks = facts::block_order(&facts_bytes)?;
    if blocks.is_empty() {
        return Err("facts.json lists no blocks".into());
    }
    counts.blocks = blocks.len();

    // Shape: one entry per block, each with a `schema` and its `states`.
    let states_bytes = read(&out.join("model-states.json"))?;
    let states: serde_json::Value =
        serde_json::from_slice(&states_bytes).map_err(|e| format!("model-states.json: {e}"))?;
    let blocks_with_states = states
        .as_object()
        .ok_or("model-states.json: not an object")?;
    if blocks_with_states.is_empty() {
        return Err("model-states.json lists no blocks".into());
    }
    let mut states_total = 0;
    for (name, entry) in blocks_with_states {
        let listed = entry
            .get("states")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| format!("model-states.json: {name} has no states"))?;
        if listed.is_empty() {
            return Err(format!("model-states.json: {name} lists no states"));
        }
        states_total += listed.len();
    }
    counts.states = states_total;

    // The expanded gameplay table is tens of megabytes; parsing it on a phone
    // costs more than it proves. Truncation is what this must catch, so check
    // that it is a list that opens and closes and is the size it should be.
    let gameplay = out.join("gameplay-facts.json");
    let size = fs::metadata(&gameplay)
        .map_err(|e| format!("{}: {}", gameplay.display(), explain(&e)))?
        .len();
    if size < 1_000_000 {
        return Err(format!(
            "{}: {size} bytes, smaller than any complete table",
            gameplay.display()
        ));
    }
    // Shape: {"states":[...]}. A truncated file loses the closing brace or the
    // opening of the list, and both are visible from the two ends.
    let (first, last) = ends(&gameplay)?;
    if first != b'{' || last != b'}' {
        return Err(format!(
            "{}: does not start and end like an object",
            gameplay.display()
        ));
    }
    if !head_bytes(&gameplay, 12)?.starts_with(br#"{"states":["#) {
        return Err(format!("{}: does not open with states", gameplay.display()));
    }

    for table in [
        "occlusion.nbt",
        "cached-collision.nbt",
        "movement-properties.nbt",
        "worldgen/blocks/cached-support.nbt",
        BLOCK_PROPERTIES,
    ] {
        let path = out.join(table);
        let bytes = read(&path)?;
        let entries = nbt::entries(&bytes)?;
        if !entries.iter().any(|entry| entry.name == "states") {
            return Err(format!("{table}: no states list"));
        }
        counts.tables += 1;
    }

    counts.assets = count_files(&out.join("assets"));
    if counts.assets == 0 {
        return Err("assets/ is empty".into());
    }
    counts.worldgen = count_files(&out.join("worldgen"));
    if counts.worldgen == 0 {
        return Err("worldgen/ is empty".into());
    }
    Ok(counts)
}
/// The four tables whose `states` list is one shared blob.
const TRAILER_TABLES: [&str; 4] = [
    "occlusion.nbt",
    "cached-collision.nbt",
    "movement-properties.nbt",
    "worldgen/blocks/cached-support.nbt",
];

/// `block-properties.nbt`, which also carries an `occlusion` copy.
const BLOCK_PROPERTIES: &str = "worldgen/blocks/block-properties.nbt";

/// Assemble `bundle` plus the player's own `client_jar` into `out`.
///
/// `store` is the asset store (`objects/`), `index` its index file, both of
/// which `hzr-download` fetches. `out` must be empty or absent.
pub fn assemble(
    bundle: &Path,
    client_jar: &Path,
    index: &Path,
    store: &Path,
    out: &Path,
) -> Result<Assembled, String> {
    let mut done = Assembled::default();
    if out.is_dir() && fs::read_dir(out).map_err(|e| e.to_string())?.next().is_some() {
        return Err(format!("{} is not empty", out.display()));
    }
    fs::create_dir_all(out).map_err(|e| format!("{}: {}", out.display(), explain(&e)))?;

    // 1) Unpack, then put the split tables back together.
    done.unpacked = hzr_pack::extract(bundle, out)?;
    let shared = out.join("_shared");
    let states = read(&shared.join("states.bin"))?;

    for table in TRAILER_TABLES {
        let path = out.join(table);
        let body = read(&path)?;
        let whole = nbt::restore(&body, &states)?;
        done.bytes += write(&path, &whole)?;
        done.restored_tables += 1;
    }
    // block-properties: its own states trailer, then the occlusion copy, which
    // is rebuilt from the occlusion table rather than carried.
    let bp = out.join(BLOCK_PROPERTIES);
    let with_states = nbt::restore(&read(&bp)?, &states)?;
    let index_bytes = read(&shared.join("occlusion.index"))?;
    let key_index = u32::from_le_bytes([
        *index_bytes.first().ok_or("occlusion.index is empty")?,
        index_bytes[1],
        index_bytes[2],
        index_bytes[3],
    ]) as usize;
    let occlusion = read(&out.join("occlusion.nbt"))?;
    let rebuilt = nbt::restore_key(
        &with_states,
        &nbt::KeySpan {
            index: key_index,
            bytes: nbt::occlusion_key(&occlusion)?,
        },
    )?;
    done.bytes += write(&bp, &rebuilt)?;
    done.restored_tables += 1;

    finish(out, client_jar, index, store, &mut done)?;
    Ok(done)
}

/// Everything after the tables are in place: the asset index, the assets and
/// data trees, the derived tables and the manifests. The bundle path and the
/// jar-only path share this half.
fn finish(
    out: &Path,
    client_jar: &Path,
    index: &Path,
    store: &Path,
    done: &mut Assembled,
) -> Result<(), String> {
    let shared = out.join("_shared");
    // The duplicate collision table is the root one, again.
    let collision = read(&out.join("cached-collision.nbt"))?;
    done.bytes += write(&out.join(staging::DUPLICATE), &collision)?;

    // The asset index is the index the download already produced.
    done.bytes += write(&out.join("asset-index.json"), &read(index)?)?;

    // 2) Derive assets, data and worldgen from the player's own copy.
    let index_bytes = read(index)?;
    let assets = hzr_fetch::AssetIndex::parse(&index_bytes)?;
    let extracted = jar::extract_jar(client_jar, out, Some(&assets), Some(store))?;
    // The worldgen trees (including the 1,511 structure templates) are a view of
    // the jar's data folder; the CLI's extract does this too.
    let mirrored = jar::mirror_worldgen(out)?;
    done.derived_files = extracted.files + mirrored;
    let imported = jar::import_indexed(
        out,
        &assets,
        store,
        &[
            jar::Import::Fonts,
            jar::Import::Languages,
            jar::Import::Panorama,
            jar::Import::Packs,
        ],
    )?;
    done.imported_files = imported.files;

    // 3) Rebuild the tables the bundle left out.
    let order = shared.join("block-order.json");
    let state_table = shared.join("model-states.json");
    let order_doc: serde_json::Value =
        serde_json::from_slice(&read(&order)?).map_err(|e| format!("{order:?}: {e}"))?;
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
    let defaults = order_doc
        .get("defaults")
        .and_then(|value| value.as_object())
        .map(|map| {
            map.iter()
                .filter_map(|(name, value)| value.as_u64().map(|id| (name.clone(), id)))
                .collect::<std::collections::BTreeMap<String, u64>>()
        });
    let states_table = facts::from_model_states(&read(&state_table)?)?;
    let built = facts::build(&facts::Table {
        order: names,
        states: states_table,
        defaults,
    })?;
    let facts_bytes = serde_json::to_vec(&built).map_err(|e| e.to_string())?;
    done.bytes += write(&out.join("facts.json"), &facts_bytes)?;

    // model-states, from the facts just built and the jar's blockstates.
    let derived_states = model_states::derive(&facts_bytes)?;
    let mut derived_states = match derived_states {
        serde_json::Value::Object(map) => map,
        _ => return Err("model-states derivation did not produce an object".into()),
    };
    let (client_only, unsupported) = model_states::synthesize_from_blockstates(
        &out.join("assets/minecraft/blockstates"),
        &derived_states,
    )?;
    for (name, entry) in client_only {
        derived_states.insert(name, entry);
    }
    if !unsupported.is_empty() {
        return Err(format!(
            "{} client-only blocks could not be rebuilt from their blockstate files",
            unsupported.len()
        ));
    }
    let states_bytes = serde_json::to_vec(&serde_json::Value::Object(derived_states))
        .map_err(|e| e.to_string())?;
    done.bytes += write(&out.join("model-states.json"), &states_bytes)?;

    // gameplay-facts, expanded from its dictionary form.
    let expanded = gameplay::expand(&read(&shared.join("gameplay.compact.json"))?)?;
    let gameplay_bytes = serde_json::to_vec(&expanded).map_err(|e| e.to_string())?;
    done.bytes += write(&out.join("gameplay-facts.json"), &gameplay_bytes)?;

    // The 20 worldgen tables, from the jar's data tree and the facts.
    let written = worldgen::write_all(out, out)?;
    done.worldgen_bundles = written.len();
    done.bytes += written.iter().map(|(_, bytes)| *bytes as u64).sum::<u64>();
    let flags = worldgen::block_flags(&facts_bytes)?;
    let flags_bytes = serde_json::to_vec(&flags).map_err(|e| e.to_string())?;
    done.bytes += write(&out.join("worldgen/block_flags.json"), &flags_bytes)?;

    // The launcher's resource manifest. The engine only reads `schema` and
    // `minecraft`; the hashes in the old Java bundle are not needed after the
    // tables are derived on device.
    let manifest = serde_json::json!({
        "schema": 7,
        "minecraft": "26.3",
        "data_version": 5023,
    });
    done.bytes += write(
        &out.join("horizon-resources.json"),
        &serde_json::to_vec(&manifest).map_err(|e| e.to_string())?,
    )?;
    let prepare = serde_json::json!({ "output_version": 4 });
    done.bytes += write(
        &out.join("horizon-prepare.json"),
        &serde_json::to_vec(&prepare).map_err(|e| e.to_string())?,
    )?;

    Ok(())
}

/// Assemble `out` from the player's own `client.jar`, with no shipped bundle:
/// every table is generated on device first, then the same asset and derived
/// pipeline runs. This is the path that lets the app stop distributing
/// Minecraft's derived data.
#[cfg(target_os = "ios")]
unsafe extern "C" {
    fn NCLRAssemblyProgress(done: i32, total: i32);
}

fn assembly_progress(done: usize, total: usize) {
    #[cfg(target_os = "ios")]
    unsafe {
        NCLRAssemblyProgress(done as i32, total as i32);
    }
    #[cfg(not(target_os = "ios"))]
    let _ = (done, total);
}

pub fn assemble_from_jar(
    client_jar: &Path,
    index: &Path,
    store: &Path,
    out: &Path,
) -> Result<Assembled, String> {
    let mut done = Assembled::default();
    if out.is_dir() && fs::read_dir(out).map_err(|e| e.to_string())?.next().is_some() {
        return Err(format!("{} is not empty", out.display()));
    }
    fs::create_dir_all(out).map_err(|e| format!("{}: {}", out.display(), explain(&e)))?;
    tables::generate_with_progress(client_jar, out, &|done, total| {
        assembly_progress(done, total)
    })?;
    restore_generated_tables(out)?;
    finish(out, client_jar, index, store, &mut done)?;
    Ok(done)
}

/// The exporter writes the same split form the shipped bundle used: the four
/// shape tables and `block-properties` carry a shared `states` trailer, and
/// `occlusion`/`cached-collision` are the whole tables they embed. Put them
/// back together exactly as the bundle path does.
pub fn restore_generated_tables(out: &Path) -> Result<(), String> {
    let shared = out.join("_shared");
    let states = read(&shared.join("states.bin"))?;
    for table in TRAILER_TABLES {
        let path = out.join(table);
        let body = read(&path)?;
        let whole = nbt::restore(&body, &states)?;
        write(&path, &whole)?;
    }
    let bp = out.join(BLOCK_PROPERTIES);
    let with_states = nbt::restore(&read(&bp)?, &states)?;
    let index_bytes = read(&shared.join("occlusion.index"))?;
    let key_index = u32::from_le_bytes([
        *index_bytes.first().ok_or("occlusion.index is empty")?,
        index_bytes[1],
        index_bytes[2],
        index_bytes[3],
    ]) as usize;
    let occlusion = read(&out.join("occlusion.nbt"))?;
    let rebuilt = nbt::restore_key(
        &with_states,
        &nbt::KeySpan {
            index: key_index,
            bytes: nbt::occlusion_key(&occlusion)?,
        },
    )?;
    write(&bp, &rebuilt)?;
    let collision = read(&out.join("cached-collision.nbt"))?;
    write(&out.join(staging::DUPLICATE), &collision)?;
    Ok(())
}

/// `assemble_from_jar`, staged and verified like [`assemble_clean`].
pub fn assemble_from_jar_clean(
    client_jar: &Path,
    index: &Path,
    store: &Path,
    out: &Path,
) -> Result<Assembled, String> {
    let parent = out
        .parent()
        .ok_or_else(|| format!("{}: no parent folder", out.display()))?;
    let name = out
        .file_name()
        .ok_or_else(|| format!("{}: no folder name", out.display()))?
        .to_string_lossy()
        .into_owned();
    fs::create_dir_all(parent).map_err(|e| format!("{}: {}", parent.display(), explain(&e)))?;
    let staging = parent.join(format!(".{name}.assembling"));
    for leftover in [&staging, &parent.join(format!(".{name}.previous"))] {
        if leftover.exists() {
            fs::remove_dir_all(leftover)
                .map_err(|e| format!("{}: {}", leftover.display(), explain(&e)))?;
        }
    }
    let done = assemble_from_jar(client_jar, index, store, &staging)?;
    forget_shared(&staging)?;
    verify(&staging)?;
    swap_into_place(&staging, out)?;
    Ok(done)
}

/// Where the assembled tree keeps the shipped bundle's leftovers. Removed after
/// a successful assembly so the folder is exactly what the engine expects.
pub fn forget_shared(out: &Path) -> Result<(), String> {
    let shared = out.join("_shared");
    if shared.is_dir() {
        fs::remove_dir_all(&shared).map_err(|e| format!("{}: {}", shared.display(), explain(&e)))?;
    }
    Ok(())
}

/// Assemble into a sibling folder, then swap it into place.
///
/// The shell creates the destination before it calls this, and a failed first
/// run can leave files behind, so assembling in place would refuse the retry.
/// Nothing at `out` is touched until the staging folder is complete.
pub fn assemble_clean(
    bundle: &Path,
    client_jar: &Path,
    index: &Path,
    store: &Path,
    out: &Path,
) -> Result<Assembled, String> {
    let parent = out
        .parent()
        .ok_or_else(|| format!("{}: no parent folder", out.display()))?;
    let name = out
        .file_name()
        .ok_or_else(|| format!("{}: no folder name", out.display()))?
        .to_string_lossy()
        .into_owned();
    fs::create_dir_all(parent).map_err(|e| format!("{}: {}", parent.display(), explain(&e)))?;
    let staging = parent.join(format!(".{name}.assembling"));
    for leftover in [&staging, &parent.join(format!(".{name}.previous"))] {
        if leftover.exists() {
            fs::remove_dir_all(leftover).map_err(|e| format!("{}: {}", leftover.display(), explain(&e)))?;
        }
    }

    let done = assemble(bundle, client_jar, index, store, &staging)?;
    forget_shared(&staging)?;
    // A tree that is present but broken must not replace one that works.
    verify(&staging)?;
    swap_into_place(&staging, out)?;
    Ok(done)
}

/// Remove what the download left behind once the tree is assembled.
///
/// A first run needs about 377 MiB at its peak: 142 MiB of downloads and 253 MiB
/// of assembled tree. The downloads are regenerable from Mojang's CDN, so a phone
/// can have that space back afterwards.
///
/// Refuses unless the folders look like the ones this crate produced, so a wrong
/// path cannot delete something else.
pub fn forget_downloads(jar_dir: &Path, store: &Path) -> Result<u64, String> {
    let jar = jar_dir.join("client.jar");
    if jar_dir.exists() && !jar.is_file() {
        return Err(format!(
            "{}: no client.jar, refusing to delete it",
            jar_dir.display()
        ));
    }
    if store.exists() && !store.join("objects").is_dir() {
        return Err(format!(
            "{}: no objects folder, refusing to delete it",
            store.display()
        ));
    }

    let mut freed = 0;
    for dir in [jar_dir, store] {
        if !dir.exists() {
            continue;
        }
        freed += dir_size(dir);
        fs::remove_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), explain(&e)))?;
    }
    Ok(freed)
}

fn dir_size(root: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total += dir_size(&path);
        } else if let Ok(meta) = fs::metadata(&path) {
            total += meta.len();
        }
    }
    total
}
/// Move `staging` to `out`, keeping whatever was at `out` until the move lands.
///
/// A failed swap restores the old folder, so a first run that dies halfway never
/// leaves the player with nothing.
pub fn swap_into_place(staging: &Path, out: &Path) -> Result<(), String> {
    let parent = out
        .parent()
        .ok_or_else(|| format!("{}: no parent folder", out.display()))?;
    let name = out
        .file_name()
        .ok_or_else(|| format!("{}: no folder name", out.display()))?
        .to_string_lossy()
        .into_owned();
    let backup = parent.join(format!(".{name}.previous"));
    if backup.exists() {
        fs::remove_dir_all(&backup).map_err(|e| format!("{}: {}", backup.display(), explain(&e)))?;
    }
    if out.exists() {
        fs::rename(out, &backup).map_err(|e| format!("{}: {}", out.display(), explain(&e)))?;
    }
    if let Err(error) = fs::rename(staging, out) {
        if backup.exists() {
            let _ = fs::rename(&backup, out);
        }
        return Err(format!("{}: {error}", out.display()));
    }
    if backup.exists() {
        fs::remove_dir_all(&backup).map_err(|e| format!("{}: {}", backup.display(), explain(&e)))?;
    }
    Ok(())
}

/// The files a finished assembly must have, for a caller that wants to check
/// before launching.
pub fn missing_from(out: &Path) -> Vec<PathBuf> {
    let mut missing = Vec::new();
    for name in [
        "facts.json",
        "model-states.json",
        "gameplay-facts.json",
        "horizon-resources.json",
    ] {
        if !out.join(name).is_file() {
            missing.push(PathBuf::from(name));
        }
    }
    if !out.join("assets").is_dir() {
        missing.push(PathBuf::from("assets"));
    }
    missing
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hzr-device-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_swap_replaces_an_existing_tree_and_leaves_no_backup() {
        let root = scratch("swap");
        let out = root.join("resources");
        fs::create_dir_all(&out).unwrap();
        fs::write(out.join("old.txt"), b"old").unwrap();
        let staging = root.join(".resources.assembling");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("new.txt"), b"new").unwrap();

        swap_into_place(&staging, &out).unwrap();
        assert!(out.join("new.txt").is_file());
        assert!(!out.join("old.txt").exists());
        assert!(!staging.exists());
        assert!(!root.join(".resources.previous").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_failed_swap_keeps_what_the_player_already_had() {
        let root = scratch("restore");
        let out = root.join("resources");
        fs::create_dir_all(&out).unwrap();
        fs::write(out.join("old.txt"), b"old").unwrap();
        // No staging folder: the move cannot land.
        let staging = root.join(".resources.assembling");

        let error = swap_into_place(&staging, &out).unwrap_err();
        assert!(error.contains("resources"), "{error}");
        assert!(out.join("old.txt").is_file(), "the old tree was lost");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_swap_makes_the_folder_when_nothing_was_there() {
        let root = scratch("fresh");
        let out = root.join("resources");
        let staging = root.join(".resources.assembling");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("new.txt"), b"new").unwrap();
        swap_into_place(&staging, &out).unwrap();
        assert!(out.join("new.txt").is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_tree_is_reported_by_name() {
        let root = scratch("missing");
        let missing = missing_from(&root);
        let names: Vec<String> = missing
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        assert!(names.contains(&"facts.json".to_string()), "{names:?}");
        assert!(names.contains(&"assets".to_string()), "{names:?}");

        for name in ["facts.json", "model-states.json", "gameplay-facts.json", "horizon-resources.json"] {
            fs::write(root.join(name), b"{}").unwrap();
        }
        fs::create_dir_all(root.join("assets")).unwrap();
        assert!(missing_from(&root).is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn forget_shared_removes_only_that_folder() {
        let root = scratch("shared");
        let shared = root.join("_shared");
        fs::create_dir_all(&shared).unwrap();
        fs::write(shared.join("states.bin"), b"x").unwrap();
        fs::write(root.join("facts.json"), b"{}").unwrap();

        forget_shared(&root).unwrap();
        assert!(!shared.exists());
        assert!(root.join("facts.json").is_file());
        // A tree without the folder is not an error.
        forget_shared(&root).unwrap();
        let _ = fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod verify_tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hzr-verify-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_missing_table_is_named() {
        let root = scratch("missing");
        let error = verify(&root).unwrap_err();
        assert!(error.contains("facts.json"), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn facts_without_blocks_is_refused() {
        let root = scratch("empty-facts");
        fs::write(root.join("facts.json"), br#"{"states":[]}"#).unwrap();
        let error = verify(&root).unwrap_err();
        assert!(error.contains("no blocks"), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_truncated_gameplay_table_is_refused_before_it_is_parsed() {
        let root = scratch("short-gameplay");
        fs::write(root.join("facts.json"), br#"{"states":[["minecraft:air"]]}"#).unwrap();
        fs::write(root.join("model-states.json"), br#"{"minecraft:air":{"states":[{}]}}"#).unwrap();
        fs::write(root.join("gameplay-facts.json"), b"[]").unwrap();
        let error = verify(&root).unwrap_err();
        assert!(error.contains("smaller than any complete table"), "{error}");
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn an_assembly_that_fails_leaves_the_old_tree_alone() {
        // What a full disk or a broken bundle does: the work stops partway.
        // The player's working tree must still be there afterwards.
        let root = scratch("failed-assembly");
        let out = root.join("resources");
        fs::create_dir_all(&out).unwrap();
        fs::write(out.join("facts.json"), b"the tree that works").unwrap();

        let bundle = root.join("ship.hzr3");
        fs::write(&bundle, b"not an HZR3 archive").unwrap();
        let jar = root.join("client.jar");
        fs::write(&jar, b"not a jar").unwrap();
        let index = root.join("34.json");
        fs::write(&index, b"{}").unwrap();
        let store = root.join("objects");
        fs::create_dir_all(&store).unwrap();

        let error = assemble_clean(&bundle, &jar, &index, &store, &out).unwrap_err();
        assert!(!error.is_empty());

        // The old tree survived, byte for byte.
        assert_eq!(
            fs::read(out.join("facts.json")).unwrap(),
            b"the tree that works"
        );
        // And nothing replaced it.
        assert!(!root.join(".resources.previous").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_failed_assembly_does_not_grow_without_bound() {
        // The staging folder is removed at the start of the next attempt, so a
        // player who retries after a failure does not accumulate copies.
        let root = scratch("retry-staging");
        let out = root.join("resources");
        fs::create_dir_all(&out).unwrap();
        fs::write(out.join("marker"), b"keep").unwrap();
        let bundle = root.join("ship.hzr3");
        fs::write(&bundle, b"not an HZR3 archive").unwrap();
        let jar = root.join("client.jar");
        fs::write(&jar, b"x").unwrap();
        let index = root.join("34.json");
        fs::write(&index, b"{}").unwrap();
        let store = root.join("objects");
        fs::create_dir_all(&store).unwrap();

        let _ = assemble_clean(&bundle, &jar, &index, &store, &out);
        let staging = root.join(".resources.assembling");
        let first = if staging.exists() {
            (fs::read_dir(&staging).unwrap().count(), true)
        } else {
            (0, false)
        };
        let _ = assemble_clean(&bundle, &jar, &index, &store, &out);
        let second = if staging.exists() {
            (fs::read_dir(&staging).unwrap().count(), true)
        } else {
            (0, false)
        };
        assert_eq!(first, second, "the staging folder grew across attempts");
        assert!(out.join("marker").exists());
        let _ = fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod explain_tests {
    use super::*;

    #[test]
    fn a_full_disk_says_so() {
        for code in [28, 112, 39] {
            let text = explain(&std::io::Error::from_raw_os_error(code));
            assert!(text.contains("no space left"), "{code}: {text}");
        }
    }

    #[test]
    fn a_refused_write_says_so() {
        for code in [13, 5] {
            let text = explain(&std::io::Error::from_raw_os_error(code));
            assert!(text.contains("may not write there"), "{code}: {text}");
        }
    }

    #[test]
    fn anything_else_keeps_the_original_text() {
        let text = explain(&std::io::Error::from_raw_os_error(2));
        assert!(text.contains("os error 2"), "{text}");
        let bare = explain(&std::io::Error::new(std::io::ErrorKind::Other, "custom"));
        assert!(bare.contains("custom"), "{bare}");
    }
}

#[cfg(test)]
mod download_cleanup_tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hzr-clean-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_downloads_are_removed_and_nothing_else() {
        let root = scratch("removes");
        let jar_dir = root.join("mc");
        fs::create_dir_all(&jar_dir).unwrap();
        fs::write(jar_dir.join("client.jar"), vec![0u8; 1024]).unwrap();
        let store = root.join("assets");
        fs::create_dir_all(store.join("objects")).unwrap();
        fs::write(store.join("objects").join("abc"), vec![0u8; 2048]).unwrap();
        let keep = root.join("resources");
        fs::create_dir_all(&keep).unwrap();
        fs::write(keep.join("facts.json"), b"keep me").unwrap();

        let freed = forget_downloads(&jar_dir, &store).unwrap();
        assert_eq!(freed, 1024 + 2048);
        assert!(!jar_dir.exists());
        assert!(!store.exists());
        assert!(keep.join("facts.json").is_file(), "the tree was deleted");

        // Nothing left to remove is not an error.
        assert_eq!(forget_downloads(&jar_dir, &store).unwrap(), 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_folder_that_is_not_ours_is_refused() {
        let root = scratch("refuses");
        let not_jar = root.join("pictures");
        fs::create_dir_all(&not_jar).unwrap();
        fs::write(not_jar.join("holiday.jpg"), b"x").unwrap();
        let store = root.join("assets");
        fs::create_dir_all(&store).unwrap();

        let error = forget_downloads(&not_jar, &store).unwrap_err();
        assert!(error.contains("no client.jar"), "{error}");
        assert!(not_jar.join("holiday.jpg").is_file(), "it deleted anyway");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_store_without_objects_is_refused() {
        let root = scratch("refuses-store");
        let jar_dir = root.join("mc");
        fs::create_dir_all(&jar_dir).unwrap();
        fs::write(jar_dir.join("client.jar"), b"jar").unwrap();
        let store = root.join("documents");
        fs::create_dir_all(&store).unwrap();
        fs::write(store.join("notes.txt"), b"mine").unwrap();

        let error = forget_downloads(&jar_dir, &store).unwrap_err();
        assert!(error.contains("no objects folder"), "{error}");
        assert!(store.join("notes.txt").is_file());
        let _ = fs::remove_dir_all(root);
    }
}
