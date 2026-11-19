//! The world generator's bundles, rebuilt from the jar's own data.
//!
//! `horizon-prepare`'s `worldgen_bundles` writes `worldgen/<name>.json` by taking
//! every `data/minecraft/<path>/**/*.json` and merging it into one object keyed
//! `minecraft:<path>`. Nothing in it comes from Java: the inputs are the jar's
//! data files, which `hzr-fetch extract` already produced. So this step runs on
//! the device, and the 19 bundle files need not be transferred.

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// `(bundle name, the `data/minecraft` path it merges)`, as `horizon-prepare`
/// lists them. Order is the preparer's.
pub const BUNDLES: [(&str, &str); 19] = [
    ("noises", "worldgen/noise"),
    ("densities", "worldgen/density_function"),
    ("rules", "worldgen/material_rule"),
    ("conditions", "worldgen/material_condition"),
    ("settings", "worldgen/noise_settings"),
    ("carvers", "worldgen/carver"),
    ("biomes", "worldgen/biome"),
    ("world_presets", "worldgen/world_preset"),
    ("features", "worldgen/feature"),
    ("placed_features", "worldgen/placed_feature"),
    ("state_providers", "worldgen/block_state_provider"),
    ("biome_tags", "tags/worldgen/biome"),
    ("feature_tags", "tags/worldgen/feature"),
    ("block_tags", "tags/block"),
    ("fluid_tags", "tags/fluid"),
    ("processor_lists", "worldgen/processor_list"),
    ("template_pools", "worldgen/template_pool"),
    ("structures", "worldgen/structure"),
    ("structure_sets", "worldgen/structure_set"),
];

fn collect(dir: &Path, base: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if kind.is_dir() {
            collect(&path, base, out)?;
        } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "json") {
            let relative = path
                .strip_prefix(base)
                .map_err(|_| format!("{}: outside {}", path.display(), base.display()))?;
            let key = relative
                .with_extension("")
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push((format!("minecraft:{key}"), path));
        }
    }
    Ok(())
}

/// One bundle: every file under `data_root/data/minecraft/<path>`, merged.
///
/// Each value is the file's **own text**, as a JSON string 鈥?that is what
/// `horizon-prepare`'s `json_resources` produces (`{"minecraft:<path>": text}`),
/// and the client parses it back. Emitting the parsed object instead would be
/// equivalent data in a different container, and is not what the engine reads.
pub fn bundle(data_root: &Path, path: &str) -> Result<Value, String> {
    let base: PathBuf = data_root.join("data/minecraft").join(path);
    if !base.is_dir() {
        return Err(format!("{}: the jar carried no {}", base.display(), path));
    }
    let mut files = Vec::new();
    collect(&base, &base, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut merged = Map::new();
    for (key, file) in files {
        let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        merged.insert(key, Value::String(text));
    }
    Ok(Value::Object(merged))
}

/// Every bundle, written to `<out>/worldgen/<name>.json`.
///
/// Returns each bundle's name and byte length.
pub fn write_all(data_root: &Path, out: &Path) -> Result<Vec<(String, usize)>, String> {
    let directory = out.join("worldgen");
    std::fs::create_dir_all(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    let mut written = Vec::with_capacity(BUNDLES.len());
    for (name, path) in BUNDLES {
        let merged = bundle(data_root, path)?;
        let text = serde_json::to_vec(&merged).map_err(|e| e.to_string())?;
        let target = directory.join(format!("{name}.json"));
        std::fs::write(&target, &text).map_err(|e| format!("{}: {e}", target.display()))?;
        written.push((name.to_string(), text.len()));
    }
    Ok(written)
}

/// `block_flags.json`: `[isAir, hasFluid]` per state, straight from `facts.json`.
pub fn block_flags(facts: &[u8]) -> Result<Value, String> {
    let root: Value = serde_json::from_slice(facts).map_err(|e| format!("facts.json: {e}"))?;
    let states = root
        .get("states")
        .and_then(Value::as_array)
        .ok_or("facts.json has no states")?;
    let mut flags = Vec::with_capacity(states.len());
    for (index, row) in states.iter().enumerate() {
        let row = row
            .as_array()
            .ok_or_else(|| format!("facts.json states[{index}] is not an array"))?;
        let air = row
            .get(1)
            .ok_or_else(|| format!("facts.json states[{index}] is short"))?;
        let fluid = row
            .get(2)
            .ok_or_else(|| format!("facts.json states[{index}] is short"))?;
        flags.push(Value::Array(vec![air.clone(), fluid.clone()]));
    }
    Ok(Value::Array(flags))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hzr-fetch-worldgen-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(root: &Path, relative: &str, body: &str) {
        let path = root.join("data/minecraft").join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn a_bundle_merges_its_tree_under_minecraft_keys() {
        let root = scratch("merge");
        put(&root, "worldgen/biome/plains.json", r#"{"temperature":0.8}"#);
        put(&root, "worldgen/biome/taiga.json", r#"{"temperature":0.25}"#);
        let merged = bundle(&root, "worldgen/biome").unwrap();
        let map = merged.as_object().unwrap();
        assert_eq!(map.len(), 2);
        // The value is the file's own text, as the preparer writes it.
        assert_eq!(merged["minecraft:plains"], "{\"temperature\":0.8}");
        assert_eq!(merged["minecraft:taiga"], "{\"temperature\":0.25}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn nested_paths_keep_their_slash_form() {
        let root = scratch("nested");
        put(
            &root,
            "worldgen/structure/village/plains.json",
            r#"{"type":"minecraft:village"}"#,
        );
        let merged = bundle(&root, "worldgen/structure").unwrap();
        assert!(merged.get("minecraft:village/plains").is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_tree_is_an_error_not_an_empty_bundle() {
        let root = scratch("missing");
        assert!(bundle(&root, "worldgen/biome").is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn block_flags_are_the_two_fact_columns() {
        let facts = br#"{"states":[["a",true,false,"x",{}],["b",false,true,"y",{}]]}"#;
        assert_eq!(
            block_flags(facts).unwrap(),
            serde_json::json!([[true, false], [false, true]])
        );
        assert!(block_flags(br#"{"states":[["a"]]}"#).is_err());
    }
}
