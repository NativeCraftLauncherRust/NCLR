//! The final `mob-data.json` export: `states` from the state/path module, the
//! predicate bits from `mob_flags`, the heights and dynamic shapes from
//! `mob_height`, and `placements`/`animations` from `render`.
//!
//! `VanillaMobDataExporter` writes exactly:
//!
//! ```text
//! {"states":[["<PathType>",<flags>,<height>], ...],
//!  "placements":{"minecraft:<entity>":{"placement":<name>,"heightmap":<name>}, ...},
//!  "animations":{"<Class>.<FIELD>":{"length":<float>,"looping":<bool>,
//!      "bones":{"<bone>":[{"target":<name>,"keyframes":[[<t>,[..],[..],<interp>], ...]}, ...]}}}}
//! ```
//!
//! `serde_json::Map` is a `BTreeMap`, so a plain `Value` cannot reproduce
//! Gson's insertion order. [`MobData`] therefore keeps the registry/declaration
//! orders it needs (`placement_order`, `animation_order`) and [`MobData::to_json`]
//! writes the file in Java's order. The floating point text comes from the
//! values themselves: the `render` module already stores each `f32` as the
//! shortest decimal that parses back to it, which is what Gson's
//! `Float.toString` writes, and the state heights are dyadic doubles.
//!
//! The state column's low pathfindable bits come from
//! `mob_states` (`VanillaMobDataExporter.states()` bits 1/2/4 are
//! `state.isPathfindable(LAND/WATER/AIR)`); bits 3..10 come from `mob_flags`
//! and the third column from `mob_height`.

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::Path;

use serde_json::{Map, Number, Value};

#[path = "mob_states.rs"]
pub mod mob_states;

#[path = "mob_flags.rs"]
pub mod mob_flags;

#[path = "mob_height.rs"]
pub mod mob_height;

#[path = "render.rs"]
pub mod render;

// ---------------------------------------------------------------------------
// states
// ---------------------------------------------------------------------------

/// `1 LAND | 2 WATER | 4 AIR | 128 empty-spawn | 256 face-sturdy-up` — the
/// bits `mob_states` owns. The remaining six bits come from `mob_flags`.
const STATE_BITS: i32 = 0b1_1000_0111;

#[derive(Debug, Clone, PartialEq)]
pub struct MobData {
    /// `[path type, flags, height]` per registry state.
    pub rows: Vec<(String, i32, f64)>,
    /// Entity ids (`minecraft:<id>`) in `BuiltInRegistries.ENTITY_TYPE` order.
    pub placement_order: Vec<String>,
    /// `"<Class>.<FIELD>"` animation ids in declaration order.
    pub animation_order: Vec<String>,
    pub placements: Value,
    pub animations: Value,
}

impl MobData {
    pub fn states_value(&self) -> Value {
        Value::Array(
            self.rows
                .iter()
                .map(|(path, flags, height)| {
                    Value::Array(vec![
                        Value::String(path.clone()),
                        Value::Number(Number::from(*flags)),
                        Value::Number(
                            Number::from_f64(*height).unwrap_or_else(|| Number::from(0)),
                        ),
                    ])
                })
                .collect(),
        )
    }

    /// The whole table as a `Value` (map keys sorted, so this is for semantic
    /// comparison; [`to_json`](Self::to_json) keeps Java's order).
    pub fn to_value(&self) -> Value {
        let mut root = Map::new();
        root.insert("states".to_string(), self.states_value());
        root.insert("placements".to_string(), self.placements.clone());
        root.insert("animations".to_string(), self.animations.clone());
        Value::Object(root)
    }

    /// The file exactly as `VanillaMobDataExporter` writes it: top-level key
    /// order states/placements/animations, placement rows in entity registry
    /// order with `placement` before `heightmap`, and animations in
    /// declaration order with `length`/`looping`/`bones`.
    pub fn to_json(&self) -> Result<String, String> {
        let states = serde_json::to_string(&self.states_value())
            .map_err(|error| error.to_string())?;
        let placements = self.placements_json()?;
        let animations = self.animations_json()?;
        Ok(format!(
            "{{\"states\":{states},\"placements\":{{{placements}}},\"animations\":{{{animations}}}}}"
        ))
    }

    fn placements_json(&self) -> Result<String, String> {
        let object = self
            .placements
            .as_object()
            .ok_or_else(|| "placements is not an object".to_string())?;
        let mut out = String::new();
        let mut seen = BTreeSet::new();
        for (index, id) in self.placement_order.iter().enumerate() {
            let row = object
                .get(id)
                .ok_or_else(|| format!("placements is missing {id}"))?;
            let row = row
                .as_object()
                .ok_or_else(|| format!("placements[{id}] is not an object"))?;
            let placement = row
                .get("placement")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("placements[{id}].placement is missing"))?;
            let heightmap = row
                .get("heightmap")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("placements[{id}].heightmap is missing"))?;
            if index > 0 {
                out.push(',');
            }
            out.push_str(&serde_json::to_string(id).map_err(|error| error.to_string())?);
            out.push_str(":{\"placement\":");
            out.push_str(
                &serde_json::to_string(placement).map_err(|error| error.to_string())?,
            );
            out.push_str(",\"heightmap\":");
            out.push_str(
                &serde_json::to_string(heightmap).map_err(|error| error.to_string())?,
            );
            out.push('}');
            seen.insert(id.clone());
        }
        if seen.len() != object.len() {
            let extra: Vec<&String> = object.keys().filter(|key| !seen.contains(*key)).collect();
            return Err(format!("placements has unlisted rows: {extra:?}"));
        }
        Ok(out)
    }

    fn animations_json(&self) -> Result<String, String> {
        let object = self
            .animations
            .as_object()
            .ok_or_else(|| "animations is not an object".to_string())?;
        let mut out = String::new();
        let mut seen = BTreeSet::new();
        for (index, id) in self.animation_order.iter().enumerate() {
            let animation = object
                .get(id)
                .ok_or_else(|| format!("animations is missing {id}"))?;
            if index > 0 {
                out.push(',');
            }
            out.push_str(&serde_json::to_string(id).map_err(|error| error.to_string())?);
            out.push(':');
            write_animation(&mut out, animation)?;
            seen.insert(id.clone());
        }
        if seen.len() != object.len() {
            let extra: Vec<&String> = object.keys().filter(|key| !seen.contains(*key)).collect();
            return Err(format!("animations has unlisted entries: {extra:?}"));
        }
        Ok(out)
    }
}

/// Gson's `Float.toString`: plain decimal for `1e-3 <= |v| < 1e7`, otherwise
/// `d.dddE±e`. The animation values are all `float`s in the Java exporter, and
/// Gson switches to scientific notation below `1e-3` (e.g. `-3.4906584E-4`),
/// while `serde_json` writes the shortest plain decimal of the f64.
fn java_float(value: f32) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() { "-0.0" } else { "0.0" }.to_string();
    }
    let negative = value.is_sign_negative();
    let magnitude = value.abs();
    let scientific = format!("{magnitude:e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .unwrap_or((scientific.as_str(), "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(|character| *character != '.').collect();
    let sign = if negative { "-" } else { "" };
    if magnitude >= 1e-3 && magnitude < 1e7 {
        let mut out = String::new();
        if exponent >= 0 {
            let integer_len = exponent as usize + 1;
            if digits.len() >= integer_len {
                out.push_str(&digits[..integer_len]);
                out.push('.');
                let fraction = &digits[integer_len..];
                out.push_str(if fraction.is_empty() { "0" } else { fraction });
            } else {
                out.push_str(&digits);
                for _ in digits.len()..integer_len {
                    out.push('0');
                }
                out.push_str(".0");
            }
        } else {
            out.push_str("0.");
            for _ in 0..(-exponent - 1) {
                out.push('0');
            }
            out.push_str(&digits);
        }
        format!("{sign}{out}")
    } else {
        let first = &digits[..1];
        let rest = &digits[1..];
        let rest = if rest.is_empty() { "0" } else { rest };
        format!("{sign}{first}.{rest}E{exponent}")
    }
}

fn write_number(out: &mut String, value: &Value) -> Result<(), String> {
    let number = value
        .as_f64()
        .ok_or_else(|| format!("expected a number, got {value:?}"))?;
    out.push_str(&java_float(number as f32));
    Ok(())
}

fn write_vec3(out: &mut String, value: &Value) -> Result<(), String> {
    let values = value
        .as_array()
        .ok_or_else(|| "vector is not an array".to_string())?;
    out.push('[');
    for (index, component) in values.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        write_number(out, component)?;
    }
    out.push(']');
    Ok(())
}

fn write_animation(out: &mut String, value: &Value) -> Result<(), String> {
    let animation = value
        .as_object()
        .ok_or_else(|| "animation is not an object".to_string())?;
    let length = animation
        .get("length")
        .ok_or_else(|| "animation.length is missing".to_string())?;
    let looping = animation
        .get("looping")
        .and_then(Value::as_bool)
        .ok_or_else(|| "animation.looping is missing".to_string())?;
    let bones = animation
        .get("bones")
        .and_then(Value::as_object)
        .ok_or_else(|| "animation.bones is missing".to_string())?;
    out.push_str("{\"length\":");
    write_number(out, length)?;
    out.push_str(",\"looping\":");
    out.push_str(if looping { "true" } else { "false" });
    out.push_str(",\"bones\":{");
    for (bone_index, (bone, channels)) in bones.iter().enumerate() {
        if bone_index > 0 {
            out.push(',');
        }
        out.push_str(&serde_json::to_string(bone).map_err(|error| error.to_string())?);
        out.push_str(":[");
        let channels = channels
            .as_array()
            .ok_or_else(|| format!("animation bone {bone} is not an array"))?;
        for (channel_index, channel) in channels.iter().enumerate() {
            if channel_index > 0 {
                out.push(',');
            }
            let channel = channel
                .as_object()
                .ok_or_else(|| format!("animation bone {bone} channel is not an object"))?;
            let target = channel
                .get("target")
                .and_then(Value::as_str)
                .ok_or_else(|| "channel.target is missing".to_string())?;
            let keyframes = channel
                .get("keyframes")
                .and_then(Value::as_array)
                .ok_or_else(|| "channel.keyframes is missing".to_string())?;
            out.push_str("{\"target\":");
            out.push_str(&serde_json::to_string(target).map_err(|error| error.to_string())?);
            out.push_str(",\"keyframes\":[");
            for (frame_index, frame) in keyframes.iter().enumerate() {
                if frame_index > 0 {
                    out.push(',');
                }
                let frame = frame
                    .as_array()
                    .ok_or_else(|| "keyframe is not an array".to_string())?;
                if frame.len() != 4 {
                    return Err(format!("keyframe has {} fields, expected 4", frame.len()));
                }
                out.push('[');
                write_number(out, &frame[0])?;
                out.push(',');
                write_vec3(out, &frame[1])?;
                out.push(',');
                write_vec3(out, &frame[2])?;
                out.push(',');
                out.push_str(
                    &serde_json::to_string(&frame[3]).map_err(|error| error.to_string())?,
                );
                out.push(']');
            }
            out.push_str("]}");
        }
        out.push(']');
    }
    out.push_str("}}");
    Ok(())
}

// ---------------------------------------------------------------------------
// export
// ---------------------------------------------------------------------------

/// Entity-type ids (`minecraft:<id>`) in `BuiltInRegistries.ENTITY_TYPE` order.
fn placement_order(jar_path: &Path) -> Result<Vec<String>, String> {
    Ok(render::entity_type_order(jar_path)?
        .into_iter()
        .map(|id| {
            if id.starts_with("minecraft:") {
                id
            } else {
                format!("minecraft:{id}")
            }
        })
        .collect())
}

/// Animation ids in Java's declaration order: class names sorted (the exporter
/// keeps the class-path names in a `TreeSet`), then `static final
/// AnimationDefinition` fields in declaration order.
fn animation_order(jar_path: &Path) -> Result<Vec<String>, String> {
    const PREFIX: &str = "net/minecraft/client/animation/definitions/";
    let file = fs::File::open(jar_path).map_err(|e| format!("{}: {e}", jar_path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", jar_path.display()))?;
    let mut classes: Vec<String> = Vec::new();
    for name in archive.file_names() {
        let Ok(name) = name else { continue };
        let name = name.to_string();
        if name.starts_with(PREFIX)
            && name.ends_with(".class")
            && !name.contains('$')
            && !name.ends_with("package-info.class")
        {
            classes.push(name);
        }
    }
    drop(archive);
    classes.sort();
    let file = fs::File::open(jar_path).map_err(|e| format!("{}: {e}", jar_path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", jar_path.display()))?;
    let mut order = Vec::new();
    for class_name in classes {
        let internal = class_name
            .strip_prefix(PREFIX)
            .and_then(|name| name.strip_suffix(".class"))
            .ok_or_else(|| format!("{class_name}: not an animation definition"))?;
        let mut entry = archive
            .by_name(&class_name)
            .map_err(|e| format!("{class_name}: {e}"))?;
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|e| format!("{class_name}: {e}"))?;
        let parsed = crate::classfile::parse(&bytes).map_err(|e| format!("{class_name}: {e}"))?;
        let simple = internal.rsplit('/').next().unwrap_or(internal);
        for field in &parsed.fields {
            if field.descriptor == "Lnet/minecraft/client/animation/AnimationDefinition;" {
                order.push(format!("{simple}.{}", field.name));
            }
        }
    }
    Ok(order)
}

/// Rebuild the whole `mob-data.json` from the player's own `client.jar`.
pub fn export_mob_data(jar_path: impl AsRef<Path>) -> Result<MobData, String> {
    let jar_path = jar_path.as_ref();
    let states = mob_states::mob_states(jar_path)?;
    let (predicate, coverage) = mob_flags::flags(jar_path)?;
    let (heights, height_gaps) = mob_height::heights(jar_path)?;
    let table_len = states.rows.len();
    if predicate.len() != table_len || heights.len() != table_len {
        return Err(format!(
            "state columns disagree: {} rows, {} predicate bits, {} heights",
            table_len,
            predicate.len(),
            heights.len()
        ));
    }
    if !height_gaps.is_empty() {
        return Err(format!("unresolved dynamic heights: {height_gaps:?}"));
    }
    if !coverage.gaps.is_empty() {
        return Err(format!("unresolved predicate flags: {:?}", coverage.gaps));
    }
    let block_ids = mob_flags::state_block_ids(jar_path)?;
    if block_ids.len() != table_len {
        return Err(format!(
            "state block ids {} != {} state rows",
            block_ids.len(),
            table_len
        ));
    }
    let mut rows = Vec::with_capacity(table_len);
    for index in 0..table_len {
        let (path, state_bits, _) = &states.rows[index];
        let mut flags = (*state_bits & STATE_BITS) | predicate[index];
        // `ScaffoldingBlock.getCollisionShape` answers `SHAPE_STABLE` at ZERO
        // (see `mob_height`); its support shape therefore has a full top face.
        // `mob_states` cannot know that while the prepared table has no shape,
        // so the face-sturdy bit is supplied here.
        if block_ids[index] == "minecraft:scaffolding" {
            flags |= 256;
        }
        rows.push((path.clone(), flags, heights[index]));
    }
    let rendered = render::export_mob_data(jar_path)?;
    let placement_order = placement_order(jar_path)?;
    let animation_order = animation_order(jar_path)?;
    let data = MobData {
        rows,
        placement_order,
        animation_order,
        placements: rendered.placements,
        animations: rendered.animations,
    };
    // Fail loudly if the orders do not cover the rendered maps.
    data.placements_json()?;
    data.animations_json()?;
    Ok(data)
}

/// Write the exact `mob-data.json`.
pub fn write_mob_data(
    jar_path: impl AsRef<Path>,
    out: impl AsRef<Path>,
) -> Result<MobData, String> {
    let data = export_mob_data(jar_path)?;
    let text = data.to_json()?;
    let out = out.as_ref();
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    fs::write(out, text).map_err(|error| format!("{}: {error}", out.display()))?;
    Ok(data)
}
