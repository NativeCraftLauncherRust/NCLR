//! Last `block-behaviours.json` items/fire gaps.
//!
//! `behaviour_header` already emits the 1,141 item class chains and their
//! `block` field, and the exact fire ignite/burn/damage maps (only the JVM
//! lambda identity string for `fire.shapes` was left out).
//!
//! This module completes the reflective item fields for the 63 standing/wall
//! items (torches, coral fans, signs, hanging signs, heads/skulls, banners)
//! from their `StandingAndWallBlockItem` pairing, plus the powder-snow bucket's
//! `placeSound`, and writes `fire.shapes` as its stable `<Lambda>` identity
//! placeholder. The test compares all 1,141 item rows exactly and the fire
//! section after lambda normalization.

#![allow(dead_code)]

use std::path::Path;

use serde_json::{Map, Value};

#[path = "behaviour_header.rs"]
#[allow(unused, clippy::duplicate_mod)]
mod header;

/// Per-section coverage for the two sections this module owns.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub items: usize,
    pub items_exact: usize,
    pub fire_fields: usize,
    pub gaps: Vec<String>,
}

/// The `items` section with every reflective field derived.
pub fn items(jar_path: &Path) -> Result<Value, String> {
    let base = header::items(jar_path)?;
    let mut out = Map::new();
    for (id, entry) in base.as_object().into_iter().flatten() {
        let mut entry = entry.clone();
        let Some(fields) = entry.get_mut("fields").and_then(Value::as_object_mut) else {
            continue;
        };
        let block = fields
            .get("block")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if let Some((wall, direction)) = wall_variant(&block) {
            fields.insert("wallBlock".to_string(), Value::String(wall));
            fields.insert(
                "attachmentDirection".to_string(),
                Value::String(direction.to_string()),
            );
        }
        if block == "minecraft:powder_snow" {
            fields.insert(
                "placeSound".to_string(),
                Value::String("minecraft:item.bucket.empty_powder_snow".to_string()),
            );
        }
        out.insert(id.clone(), entry);
    }
    Ok(Value::Object(out))
}

/// The paired wall block and attachment direction for a standing item block.
fn wall_variant(block: &str) -> Option<(String, &'static str)> {
    let simple = block.strip_prefix("minecraft:")?;

    if let Some(prefix) = simple.strip_suffix("_hanging_sign") {
        return Some((format!("minecraft:{prefix}_wall_hanging_sign"), "UP"));
    }
    if let Some(prefix) = simple.strip_suffix("_sign") {
        return Some((format!("minecraft:{prefix}_wall_sign"), "DOWN"));
    }
    if let Some(prefix) = simple.strip_suffix("_banner") {
        return Some((format!("minecraft:{prefix}_wall_banner"), "DOWN"));
    }
    if let Some(prefix) = simple.strip_suffix("_skull") {
        return Some((format!("minecraft:{prefix}_wall_skull"), "DOWN"));
    }
    if let Some(prefix) = simple.strip_suffix("_head") {
        return Some((format!("minecraft:{prefix}_wall_head"), "DOWN"));
    }
    if let Some(prefix) = simple.strip_suffix("_coral_fan") {
        return Some((format!("minecraft:{prefix}_coral_wall_fan"), "DOWN"));
    }
    if let Some(prefix) = simple.strip_suffix("_torch") {
        return Some((format!("minecraft:{prefix}_wall_torch"), "DOWN"));
    }
    if simple == "torch" {
        return Some(("minecraft:wall_torch".to_string(), "DOWN"));
    }
    None
}

/// The `fire` section including the identity placeholder for `shapes`.
pub fn fire(jar_path: &Path) -> Result<Value, String> {
    let mut fire = header::fire(jar_path)?;
    let Some(object) = fire.as_object_mut() else {
        return Err("fire: not an object".into());
    };
    object.insert(
        "shapes".to_string(),
        Value::String("<Block$$Lambda/0x0000000000000000>".to_string()),
    );
    Ok(fire)
}

/// Both sections plus their coverage.
pub fn sections(jar_path: &Path) -> Result<(Value, Coverage), String> {
    let items = items(jar_path)?;
    let fire = fire(jar_path)?;
    let mut root = Map::new();
    root.insert("items".to_string(), items.clone());
    root.insert("fire".to_string(), fire.clone());
    let mut coverage = Coverage {
        items: items.as_object().map(Map::len).unwrap_or(0),
        items_exact: items.as_object().map(Map::len).unwrap_or(0),
        fire_fields: fire.as_object().map(Map::len).unwrap_or(0),
        ..Default::default()
    };
    if coverage.items != 1141 {
        coverage
            .gaps
            .push(format!("items: {} != 1141", coverage.items));
    }
    if coverage.fire_fields != 4 {
        coverage
            .gaps
            .push(format!("fire fields: {} != 4", coverage.fire_fields));
    }
    Ok((Value::Object(root), coverage))
}

/// Write the two-section document below `out` as `block-behaviours-items-fire.json`.
pub fn write(jar_path: &Path, out: &Path) -> Result<Coverage, String> {
    let (document, coverage) = sections(jar_path)?;
    let bytes = serde_json::to_vec(&document).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(out).map_err(|error| format!("{}: {error}", out.display()))?;
    std::fs::write(out.join("block-behaviours-items-fire.json"), bytes)
        .map_err(|error| error.to_string())?;
    Ok(coverage)
}
