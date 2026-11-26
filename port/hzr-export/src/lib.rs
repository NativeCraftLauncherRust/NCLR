//! Shared core of `hzr-export`.
//!
//! The exporter used to be a single binary that pulled every table module into
//! one rustc codegen unit. With 46 path-included modules that made LLVM run out
//! of memory. It is now split:
//!
//! * this library holds the small, dependency-free helpers and the cheap
//!   jar/JSON commands;
//! * `src/main.rs` is a thin dispatcher for those cheap commands and forwards
//!   the heavy table commands to one of the per-group binaries in `src/bin/`;
//! * each `src/bin/hzr-export-<group>.rs` path-includes only the modules its own
//!   commands need (the same pattern the integration tests use), so no single
//!   rustc invocation sees all 46 modules.
//!
//! The table logic itself is untouched; the `export_*` functions below are the
//! bodies that used to live in `main.rs`.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

use serde_json::{Map, Value};

/// Pretty JSON exactly as the old binary wrote it.
pub fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("JSON is serializable")
}

/// Read every non-directory entry of a jar (zip) into memory.
pub fn read_zip(path: &str) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let file = fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("{path}: {e}"))?;
    let mut files = BTreeMap::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().map_err(|e| e.to_string())?.to_string();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).map_err(|e| e.to_string())?;
        files.insert(name, data);
    }
    Ok(files)
}

/// Serialize a `String -> String` map pretty with sorted keys and a trailing
/// newline, creating the parent directory.
pub fn write_string_map(out: &str, map: BTreeMap<String, String>) -> Result<(), String> {
    let mut object = Map::new();
    for (key, value) in map {
        object.insert(key, Value::String(value));
    }
    let text = pretty(&Value::Object(object));
    let path = Path::new(out);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(path, text + "\n").map_err(|e| format!("{out}: {e}"))
}

/// `data/minecraft/tags/<kind>/*.json` -> one pretty-printed string per tag.
pub fn export_tags(jar: &str, out: &str, kind: &str) -> Result<(), String> {
    let files = read_zip(jar)?;
    let prefix = format!("data/minecraft/tags/{kind}/");
    let mut tags = BTreeMap::new();
    for (name, data) in &files {
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        let Some(id) = rest.strip_suffix(".json") else {
            continue;
        };
        let id = format!("minecraft:{id}");
        let value: Value = serde_json::from_slice(data).map_err(|e| format!("{name}: {e}"))?;
        // The exporter stores each tag's own JSON as a string, pretty-printed.
        tags.insert(id, pretty(&value));
    }
    write_string_map(out, tags)
}

/// `damage-types.json`: each damage type's JSON as a pretty-printed string.
pub fn export_damage_types(jar: &str, out: &str) -> Result<(), String> {
    let files = read_zip(jar)?;
    let prefix = "data/minecraft/damage_type/";
    let mut types = BTreeMap::new();
    for (name, data) in &files {
        let Some(rest) = name.strip_prefix(prefix) else {
            continue;
        };
        let Some(id) = rest.strip_suffix(".json") else {
            continue;
        };
        let id = format!("minecraft:{id}");
        let value: Value = serde_json::from_slice(data).map_err(|e| format!("{name}: {e}"))?;
        types.insert(id, pretty(&value));
    }
    write_string_map(out, types)
}

/// `damage-catalog.json`: id/message/scaling/exhaustion/effects/death columns.
pub fn export_damage_catalog(jar: &str, out: &str) -> Result<(), String> {
    let files = read_zip(jar)?;
    let prefix = "data/minecraft/damage_type/";
    let mut types = Vec::new();
    for (name, data) in &files {
        let Some(rest) = name.strip_prefix(prefix) else {
            continue;
        };
        let Some(id) = rest.strip_suffix(".json") else {
            continue;
        };
        let value: Value = serde_json::from_slice(data).map_err(|e| format!("{name}: {e}"))?;
        let object = value
            .as_object()
            .ok_or_else(|| format!("{name}: not an object"))?;
        let message = object
            .get("message_id")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{name}: no message_id"))?;
        let scaling = object
            .get("scaling")
            .and_then(Value::as_str)
            .unwrap_or("when_caused_by_living_non_player");
        let exhaustion = object
            .get("exhaustion")
            .and_then(Value::as_f64)
            .unwrap_or(0.0) as f32;
        let effects = object
            .get("effects")
            .and_then(Value::as_str)
            .unwrap_or("hurt");
        let death = object
            .get("death_message_type")
            .and_then(Value::as_str)
            .unwrap_or("default");
        types.push((
            format!("minecraft:{id}"),
            message.to_owned(),
            scaling.to_owned(),
            format!("{:08x}", exhaustion.to_bits()),
            effects.to_owned(),
            death.to_owned(),
        ));
    }
    types.sort_by(|a, b| a.0.cmp(&b.0));
    let mut array = Vec::new();
    for (id, message, scaling, exhaustion, effects, death) in types {
        let mut row = Map::new();
        row.insert("id".into(), Value::String(id));
        row.insert("message_id".into(), Value::String(message));
        row.insert("exhaustion".into(), Value::String(exhaustion));
        row.insert("scaling".into(), Value::String(scaling));
        row.insert("effects".into(), Value::String(effects));
        row.insert("death_message_type".into(), Value::String(death));
        array.push(Value::Object(row));
    }
    let mut root = Map::new();
    root.insert("data_version".into(), Value::Number(5023.into()));
    root.insert("types".into(), Value::Array(array));
    let path = Path::new(out);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(
        path,
        serde_json::to_string(&Value::Object(root)).map_err(|e| e.to_string())? + "\n",
    )
    .map_err(|e| format!("{out}: {e}"))
}

/// `player-float-ids.json`: a fixed constant table.
pub fn export_player_float_ids(out: &str) -> Result<(), String> {
    let path = Path::new(out);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(path, "{\"health\":9,\"absorption\":17}\n").map_err(|e| format!("{out}: {e}"))
}

/// `_shared/occlusion.index`: a four-byte stamp.
pub fn export_occlusion_index(out: &str) -> Result<(), String> {
    let path = Path::new(out);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(path, [1u8, 0, 0, 0]).map_err(|e| format!("{out}: {e}"))
}

/// `horizon-prepare.json`: the prepare stamp.
pub fn export_prepare_stamp(out: &str) -> Result<(), String> {
    let path = Path::new(out);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(path, "{\r\n  \"output_version\": 4\r\n}\r\n").map_err(|e| format!("{out}: {e}"))
}

/// Commands handled by the library itself (no table module needed).
pub fn run_light(args: &[String]) -> Option<Result<(), String>> {
    let result = match args {
        [cmd, jar, out] if cmd == "block-tags" => export_tags(jar, out, "block"),
        [cmd, jar, out] if cmd == "damage-tags" => export_tags(jar, out, "damage_type"),
        [cmd, jar, out] if cmd == "damage-types" => export_damage_types(jar, out),
        [cmd, jar, out] if cmd == "damage-catalog" => export_damage_catalog(jar, out),
        [cmd, _jar, out] if cmd == "player-float-ids" => export_player_float_ids(out),
        [cmd, _jar, out] if cmd == "occlusion-index" => export_occlusion_index(out),
        [cmd, _jar, out] if cmd == "prepare-stamp" => export_prepare_stamp(out),
        _ => return None,
    };
    Some(result)
}
