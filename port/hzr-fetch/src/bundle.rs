//! Stages a prepared folder into what actually has to travel.
//!
//! Four kinds of work, each already proven on its own:
//!
//! 1. **Regenerated on the device**, so not shipped: `facts.json`,
//!    `model-states.json`, `gameplay-facts.json` (compacted), and the 20
//!    `worldgen/*.json` tables.
//! 2. **One shared `states` trailer** instead of five (`nbt::strip`).
//! 3. **`block-properties.nbt`'s `occlusion` copy** rebuilt from `occlusion.nbt`.
//! 4. **A duplicate `cached-collision.nbt`**: `worldgen/blocks/`'s copy is
//!    byte-identical to the root one, so only one ships.
//!
//! The staged tree plus `_shared/` is what `hzr-pack` packs; `unbundle` puts it
//! back, and every step is byte-checked.

use crate::nbt;
use std::fs;
use std::path::{Path, PathBuf};

/// Files the device regenerates from data it already has.
pub const REGENERATED: [&str; 4] = [
    "facts.json",
    "model-states.json",
    "gameplay-facts.json",
    "worldgen/block_flags.json",
];

/// The 19 `worldgen/<name>.json` bundles `hzr-fetch worldgen` rebuilds.
pub const REGENERATED_BUNDLES: [&str; 19] = [
    "worldgen/noises.json",
    "worldgen/densities.json",
    "worldgen/rules.json",
    "worldgen/conditions.json",
    "worldgen/settings.json",
    "worldgen/carvers.json",
    "worldgen/biomes.json",
    "worldgen/world_presets.json",
    "worldgen/features.json",
    "worldgen/placed_features.json",
    "worldgen/state_providers.json",
    "worldgen/biome_tags.json",
    "worldgen/feature_tags.json",
    "worldgen/block_tags.json",
    "worldgen/fluid_tags.json",
    "worldgen/processor_lists.json",
    "worldgen/template_pools.json",
    "worldgen/structures.json",
    "worldgen/structure_sets.json",
];

/// Tables whose trailing `states` list is the same 4,173,554-byte blob.
pub const TRAILER_TABLES: [&str; 5] = [
    "cached-collision.nbt",
    "occlusion.nbt",
    "movement-properties.nbt",
    "worldgen/blocks/block-properties.nbt",
    "worldgen/blocks/cached-support.nbt",
];

/// The table that is byte-identical to `cached-collision.nbt` and need not ship.
pub const DUPLICATE: &str = "worldgen/blocks/cached-collision.nbt";
/// The copy that does ship.
pub const DUPLICATE_SOURCE: &str = "cached-collision.nbt";

fn regenerated(path: &str) -> bool {
    REGENERATED.contains(&path) || REGENERATED_BUNDLES.contains(&path)
}

/// What staging did.
#[derive(Debug, Default)]
pub struct Report {
    pub files: usize,
    pub bytes: u64,
    /// The shared trailer's length (0 when the tables were absent).
    pub trailer: usize,
    pub stripped_trailers: usize,
    pub stripped_occlusion: usize,
    pub dropped_duplicate: bool,
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if kind.is_dir() {
            walk(root, &path, out)?;
        } else if kind.is_file() {
            out.push(path.strip_prefix(root).map_err(|_| "outside")?.to_path_buf());
        }
    }
    Ok(())
}

fn slash(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Stage `resources` into `out`. Returns what was dropped and what remains.
pub fn bundle(resources: &Path, out: &Path) -> Result<Report, String> {
    let mut files = Vec::new();
    walk(resources, resources, &mut files)?;
    files.sort();

    let shared = out.join("_shared");
    fs::create_dir_all(&shared).map_err(|e| format!("{}: {e}", shared.display()))?;

    let mut report = Report::default();
    let mut trailer: Option<Vec<u8>> = None;
    let mut written: Vec<(String, Vec<u8>)> = Vec::new();

    for relative in &files {
        let name = slash(relative);
        if regenerated(&name) || name == DUPLICATE {
            if name == DUPLICATE {
                report.dropped_duplicate = true;
            }
            continue;
        }
        let source = resources.join(relative);
        let mut bytes = fs::read(&source).map_err(|e| format!("{}: {e}", source.display()))?;

        if let Some(_) = TRAILER_TABLES.iter().find(|table| **table == name) {
            let (body, blob) = nbt::strip(&bytes)?;
            match &trailer {
                Some(known) if known != &blob => {
                    return Err(format!("{name}: the states trailer differs from the first table's"));
                }
                None => trailer = Some(blob),
                _ => {}
            }
            bytes = body;
            report.stripped_trailers += 1;
            if name == "worldgen/blocks/block-properties.nbt" {
                let (body, span) = nbt::strip_key(&bytes, nbt::OCCLUSION_KEY)?;
                // Only the position travels: the key itself is rebuilt from
                // occlusion.nbt, so storing it would cancel the saving.
                fs::write(shared.join("occlusion.index"), (span.index as u32).to_le_bytes())
                    .map_err(|e| e.to_string())?;
                bytes = body;
                report.stripped_occlusion = span.bytes.len();
            }
        }
        report.files += 1;
        report.bytes += bytes.len() as u64;
        written.push((name, bytes));
    }

    if let Some(blob) = &trailer {
        fs::write(shared.join("states.bin"), blob).map_err(|e| e.to_string())?;
        report.trailer = blob.len();
    }

    let duplicate = resources.join(DUPLICATE_SOURCE);
    if duplicate.is_file() && resources.join(DUPLICATE).is_file() {
        let a = fs::read(resources.join(DUPLICATE)).map_err(|e| e.to_string())?;
        let b = fs::read(&duplicate).map_err(|e| e.to_string())?;
        if a != b {
            return Err(format!("{DUPLICATE} is not the same table as {DUPLICATE_SOURCE}"));
        }
    }

    for (name, bytes) in written {
        let target = out.join(name.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        fs::write(&target, &bytes).map_err(|e| format!("{}: {e}", target.display()))?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hzr-bundle-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn nbt_table(body_payload: &[u8]) -> Vec<u8> {
        // compound: [int "a"] [list "states" of one int]
        let mut bytes = vec![10, 0, 0];
        bytes.extend_from_slice(&[3, 0, 1, b'a']);
        bytes.extend_from_slice(body_payload);
        bytes.extend_from_slice(&[9, 0, 6]);
        bytes.extend_from_slice(b"states");
        bytes.extend_from_slice(&[3, 0, 0, 0, 1, 0, 0, 0, 7]);
        bytes.push(0);
        bytes
    }

    #[test]
    fn a_staged_tree_drops_regenerated_files_and_keeps_the_rest() {
        let root = scratch("stage");
        let source = root.join("in");
        fs::create_dir_all(source.join("worldgen")).unwrap();
        fs::write(source.join("facts.json"), b"{}").unwrap();
        fs::write(source.join("model-states.json"), b"{}").unwrap();
        fs::write(source.join("gameplay-facts.json"), b"{}").unwrap();
        fs::write(source.join("worldgen/biomes.json"), b"{}").unwrap();
        fs::write(source.join("worldgen/block_flags.json"), b"[]").unwrap();
        fs::write(source.join("items.json"), b"{}").unwrap();
        let out = root.join("out");
        let report = bundle(&source, &out).unwrap();
        assert_eq!(report.files, 1);
        assert!(out.join("items.json").is_file());
        assert!(!out.join("facts.json").exists());
        assert!(!out.join("worldgen/biomes.json").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_trailer_is_written_once_and_the_tables_lose_it() {
        let root = scratch("trailer");
        let source = root.join("in");
        fs::create_dir_all(source.join("worldgen/blocks")).unwrap();
        let table = nbt_table(&[0, 0, 0, 1]);
        fs::write(source.join("occlusion.nbt"), &table).unwrap();
        fs::write(source.join("cached-collision.nbt"), &table).unwrap();
        fs::write(source.join("movement-properties.nbt"), &table).unwrap();
        fs::write(source.join("worldgen/blocks/cached-support.nbt"), &table).unwrap();
        let out = root.join("out");
        let report = bundle(&source, &out).unwrap();
        assert_eq!(report.stripped_trailers, 4);
        assert!(report.trailer > 0);
        assert!(out.join("_shared/states.bin").is_file());
        for name in ["occlusion.nbt", "cached-collision.nbt"] {
            let bytes = fs::read(out.join(name)).unwrap();
            let (_, blob) = nbt::strip(&table).unwrap();
            assert_eq!(bytes.len() as u64 + blob.len() as u64, table.len() as u64);
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_trailer_that_differs_stops_the_stage() {
        let root = scratch("differ");
        let source = root.join("in");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("occlusion.nbt"), nbt_table(&[0, 0, 0, 1])).unwrap();
        let mut other = nbt_table(&[0, 0, 0, 2]);
        // Same trailer bytes, different body: fine. Now make the trailer differ.
        let n = other.len();
        other[n - 4] = 9;
        fs::write(source.join("cached-collision.nbt"), other).unwrap();
        let error = bundle(&source, &root.join("out")).unwrap_err();
        assert!(error.contains("differs"), "{error}");
        let _ = fs::remove_dir_all(root);
    }
}
