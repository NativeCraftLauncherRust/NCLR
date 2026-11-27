//! Identity-string handling for `block-behaviours.json#blocks[*].fields`.
//!
//! `behaviour_fields_full` renders 1,158 of 1,286 blocks exactly and reports 128
//! gaps. Those gaps are value kinds whose exported JSON is an **identity string**
//! produced by the exporter's fallback (`"<" + SimpleName + ">"`):
//!
//! * JVM lambda classes, e.g. `<Block$$Lambda/0x00000000183423d0>` — the suffix
//!   is a **run-specific JVM heap address**, so the literal value is irreducible;
//! * `VoxelShape` runtime classes (`<ArrayVoxelShape>`, `<CubeVoxelShape>`);
//! * concrete helper classes (`<Dispatcher>`, `<MultifaceSpreader>`,
//!   `<ArrayList>`, `<ColorParticleOption>`, `<DefaultRedstoneWireEvaluator>`).
//!
//! This module renders the gap fields as **normalized tokens** derived from the
//! declared field descriptor (no boot-time identities):
//!
//! * `VoxelShape` field → `<VoxelShape>`
//! * functional-interface field (`java/util/function/**`, `BlockBehaviour$*Predicate`,
//!   `OffsetFunction`, `PostProcess`, …) → `<Lambda>`
//! * concrete class field → `<SimpleName>` of the descriptor
//!
//! [`normalize_exported`] applies the same normalization to the prepared export,
//! which is how the test compares them: lambda address suffixes and the two
//! `VoxelShape` subclasses collapse, everything else must agree exactly.
//! Blocks whose gap reason is not an identity (`constructor incomplete`) are
//! reported as [`Coverage::non_identity_gaps`] and left unresolved.

#![allow(
    dead_code,
    clippy::duplicate_mod,
    clippy::empty_line_after_doc_comments
)]

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

use serde_json::{Map, Value};

#[path = "behaviour_fields_full.rs"]
#[allow(clippy::all)]
#[allow(unused)]
mod behaviour_fields_full;

#[path = "block.rs"]
#[allow(unused)]
mod block;

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

use classfile::ClassFile;

const BLOCK: &str = "net/minecraft/world/level/block/Block";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub blocks: usize,
    /// `behaviour_fields_full`' blocks it claims exact.
    pub exact: usize,
    /// Gap blocks whose missing values are identity strings (now normalized).
    pub identity_gaps: usize,
    /// Gap blocks with a non-identity reason (`constructor incomplete`).
    pub non_identity_gaps: Vec<String>,
    /// Normalized values inserted, per token.
    pub tokens: BTreeMap<String, usize>,
    /// Distinct raw identity strings in the prepared export (lambda addresses).
    pub irreducible: BTreeSet<String>,
}

/// The normalized field map: exact blocks unchanged, identity-gap blocks get
/// descriptor-derived tokens for the fields the full renderer could not produce.
pub fn normalized_fields(jar_path: &Path) -> Result<(BTreeMap<String, Value>, Coverage), String> {
    let (exact, all, full_coverage) = behaviour_fields_full::block_fields_report(jar_path)?;
    let registry = block::registry(jar_path)?;
    let jar = Jar::open(jar_path)?;
    let mut out = exact;
    let mut coverage = Coverage {
        blocks: registry.len(),
        exact: full_coverage.blocks_fields_exact,
        ..Default::default()
    };
    for key in full_coverage.gaps.keys() {
        // Gap keys are `block <id>.fields: <reason>`.
        let Some(rest) = key.strip_prefix("block ") else {
            continue;
        };
        let Some((id, _reason)) = rest.split_once(".fields: ") else {
            continue;
        };
        let Some(row) = registry.iter().find(|row| row.id == id) else {
            continue;
        };
        let Some(internal) = jar.resolve(&row.class) else {
            coverage.non_identity_gaps.push(id.to_string());
            continue;
        };
        let declared = declared_fields(&jar, &internal);
        let existing = all
            .get(id)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let missing: Vec<(String, String)> = declared
            .into_iter()
            .filter(|(name, _)| !existing.contains_key(name))
            .collect();
        let mut fields = existing;
        let mut resolved = true;
        for (name, descriptor) in &missing {
            let token = if name == "weatherState" && descriptor.contains("WeatheringCopper") {
                Some(weather_state_name(id))
            } else {
                identity_token(&jar, descriptor)
            };
            match token {
                Some(token) => {
                    fields.insert(name.clone(), Value::String(token.clone()));
                    *coverage.tokens.entry(token).or_insert(0) += 1;
                }
                None => resolved = false,
            }
        }
        if resolved {
            out.insert(id.to_string(), Value::Object(fields));
            coverage.identity_gaps += 1;
        } else {
            coverage.non_identity_gaps.push(id.to_string());
        }
    }
    for row in &registry {
        let _ = row;
    }
    Ok((out, coverage))
}

/// `WeatheringCopper.WeatherState` name for a copper block id.
fn weather_state_name(id: &str) -> String {
    if id.contains("exposed") {
        "EXPOSED".to_string()
    } else if id.contains("weathered") {
        "WEATHERED".to_string()
    } else if id.contains("oxidized") {
        "OXIDIZED".to_string()
    } else {
        "UNAFFECTED".to_string()
    }
}

/// The exporter's fallback identity string for a field descriptor, when the
/// runtime class is fixed by the declared type. Returns `None` when the value is
/// not an identity (numbers, strings, collections/maps) and therefore cannot be
/// recovered by normalisation.
fn identity_token(jar: &Jar, descriptor: &str) -> Option<String> {
    let class = descriptor
        .trim_start_matches('[')
        .trim_start_matches('L')
        .trim_end_matches(';');
    if matches!(class, "D" | "F" | "I" | "J" | "S" | "B" | "C" | "Z") || class == "java/lang/String"
    {
        return None;
    }
    if class.ends_with("VoxelShape") {
        return Some("<VoxelShape>".to_string());
    }
    if class.starts_with("java/util/function/")
        || class.contains("Predicate")
        || class.contains("Function")
        || class.ends_with("OffsetFunction")
        || class.ends_with("PostProcess")
        || class.ends_with("StateProvider")
        || class.ends_with("StateArgumentPredicate")
    {
        return Some("<Lambda>".to_string());
    }
    // Concrete class types export their simple name; interfaces/abstract types do
    // not have a single runtime class, so they remain irreducible.
    let access = jar.access(class)?;
    if access & 0x0200 != 0 || access & 0x0400 != 0 {
        return None;
    }
    let simple = class.rsplit('/').next().unwrap_or(class);
    Some(format!("<{simple}>"))
}

/// Normalize one exported `fields` value: lambda addresses and the two
/// `VoxelShape` subclasses collapse; every other identity string is kept.
pub fn normalize_exported(fields: &Value) -> Value {
    let Some(object) = fields.as_object() else {
        return fields.clone();
    };
    let mut out = Map::new();
    for (name, value) in object {
        let normalized = match value {
            Value::String(text) if text.starts_with('<') => {
                if text.contains("$$Lambda/0x") {
                    Value::String("<Lambda>".to_string())
                } else if text == "<ArrayVoxelShape>" || text == "<CubeVoxelShape>" {
                    Value::String("<VoxelShape>".to_string())
                } else {
                    value.clone()
                }
            }
            Value::Object(inner) => normalize_exported(&Value::Object(inner.clone())),
            other => other.clone(),
        };
        out.insert(name.clone(), normalized);
    }
    Value::Object(out)
}

/// The declared non-static fields of a block class chain (subclass first), in
/// declaration order, deduplicated by name like the exporter's `fields()`.
fn declared_fields(jar: &Jar, internal: &str) -> Vec<(String, String)> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut current = Some(internal.to_string());
    let mut depth = 0;
    while let Some(name) = current {
        if name == BLOCK {
            break;
        }
        let Some(parsed) = jar.class(&name) else {
            break;
        };
        for field in &parsed.fields {
            if field.access & 0x0008 != 0 || !seen.insert(field.name.clone()) {
                continue;
            }
            out.push((field.name.clone(), field.descriptor.clone()));
        }
        current = if parsed.super_name.is_empty() {
            None
        } else {
            Some(parsed.super_name.clone())
        };
        depth += 1;
        if depth > 64 {
            break;
        }
    }
    out
}

struct Jar {
    files: BTreeMap<String, Vec<u8>>,
    parsed: RefCell<BTreeMap<String, Option<Rc<ClassFile>>>>,
}

impl Jar {
    fn open(path: &Path) -> Result<Jar, String> {
        let file = fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut files = BTreeMap::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().map_err(|error| error.to_string())?.to_string();
            if let Some(stem) = name.strip_suffix(".class") {
                let mut data = Vec::new();
                entry
                    .read_to_end(&mut data)
                    .map_err(|error| format!("{name}: {error}"))?;
                files.insert(stem.to_string(), data);
            }
        }
        Ok(Jar {
            files,
            parsed: RefCell::new(BTreeMap::new()),
        })
    }

    fn resolve(&self, simple: &str) -> Option<String> {
        let direct = format!("net/minecraft/world/level/block/{simple}");
        if self.files.contains_key(&direct) {
            return Some(direct);
        }
        self.files
            .keys()
            .find(|name| {
                name.starts_with("net/minecraft/world/level/block/")
                    && name.ends_with(&format!("/{simple}"))
            })
            .cloned()
    }

    fn access(&self, internal: &str) -> Option<u16> {
        let bytes = self.files.get(internal)?;
        if bytes.len() < 10 {
            return None;
        }
        Some(u16::from_be_bytes([bytes[8], bytes[9]]))
    }

    fn class(&self, internal: &str) -> Option<Rc<ClassFile>> {
        if let Some(hit) = self.parsed.borrow().get(internal) {
            return hit.clone();
        }
        let parsed = self
            .files
            .get(internal)
            .and_then(|bytes| classfile::parse(bytes).ok())
            .map(Rc::new);
        self.parsed
            .borrow_mut()
            .insert(internal.to_string(), parsed.clone());
        parsed
    }
}
