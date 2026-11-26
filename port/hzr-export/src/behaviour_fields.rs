//! Reflective instance fields of block classes, for `block-behaviours.json`.
//!
//! `VanillaBlockBehaviourExporter.fields(block, Block.class)` walks the concrete
//! class and every superclass down to (but excluding) `Block`, collecting each
//! declared **non-static** field in declaration order, and dumps its value with a
//! `value()` visitor (enums → name, `Block` → id, `BlockState` → `{state:id}`,
//! `SoundType` → record, maps/records at depth 3, grower classes → recursive
//! fields, …).
//!
//! # What this module derives exactly
//!
//! The static half of that walk is exact without running any constructor: for a
//! block whose class chain below `Block` declares **no instance fields**, the
//! exported value is `{}`. 573 of 1,286 blocks are in that class (plain `Block`,
//! `AirBlock`, `VegetationBlock`, …). Those are emitted and verified exactly.
//!
//! The rest declare fields whose values are constructor-derived
//! (`treeGrower`, `type`, `base`, `color`, `shapes`, …). Deriving those needs the
//! `value()` visitor over reconstructed instance state (constructor VM plus
//! records, sound types, tags and holders); the gap inventory lists every field
//! owner/name/type still to model, so a later task can extend `value()` without
//! re-discovering the classes.
//!
//! Items are reported as a gap here: every `BlockItem` carries a `block` field
//! whose value is the registered block id, plus a few subclass fields
//! (`wallBlock`, `attachmentDirection`, `placeSound`); getting the item id set
//! and each item's class needs the `Items.<clinit>` registration trace, which
//! lives in `items.rs` and is out of this module's scope.

#![allow(dead_code, clippy::duplicate_mod)]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

use serde_json::{Map, Value};

#[path = "block.rs"]
#[allow(unused)]
mod block;

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

use classfile::ClassFile;

/// The class whose fields stop the walk (`Block.class` in the exporter).
const BLOCK: &str = "net/minecraft/world/level/block/Block";

/// What was derived, and the field inventory still missing.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub blocks: usize,
    pub empty_fields_exact: usize,
    pub items_derived: usize,
    /// `"<class>.<field>: <descriptor>"` → number of blocks carrying it.
    pub gaps: BTreeMap<String, usize>,
}

/// One declared instance field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub owner: String,
    pub name: String,
    pub descriptor: String,
}

/// `block-behaviours.json#blocks[*].fields` for the blocks this module can derive
/// exactly (those with no instance fields below `Block`).
pub fn block_fields(jar_path: &Path) -> Result<(Map<String, Value>, Coverage), String> {
    let jar = Jar::open(jar_path)?;
    let registry = block::registry(jar_path)?;
    let mut out = Map::new();
    let mut coverage = Coverage {
        blocks: registry.len(),
        ..Default::default()
    };
    for row in &registry {
        let internal = jar.resolve(&row.class);
        let fields = match internal {
            Some(internal) => declared_fields(&jar, &internal)?,
            None => Vec::new(),
        };
        if fields.is_empty() {
            out.insert(row.id.clone(), Value::Object(Map::new()));
            coverage.empty_fields_exact += 1;
        } else {
            for field in fields {
                *coverage
                    .gaps
                    .entry(format!("{}.{}: {}", field.owner, field.name, field.descriptor))
                    .or_insert(0) += 1;
            }
        }
    }
    // Items: every one needs the item registration trace; report them as a gap
    // without inventing entries.
    let item_gaps = item_gap_classes(&jar)?;
    coverage.items_derived = 0;
    for (class, count) in item_gaps {
        *coverage
            .gaps
            .entry(format!("item {class}: subclass fields"))
            .or_insert(0) += count;
    }
    Ok((out, coverage))
}

/// Declared non-static fields from `internal` up to (excluding) `Block`, in order.
pub fn declared_fields(jar: &Jar, internal: &str) -> Result<Vec<Field>, String> {
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
            if field.access & 0x0008 != 0 {
                continue; // static
            }
            out.push(Field {
                owner: name.clone(),
                name: field.name.clone(),
                descriptor: field.descriptor.clone(),
            });
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
    Ok(out)
}

/// Item subclasses with extra fields, for the gap report (from the jar only).
fn item_gap_classes(jar: &Jar) -> Result<Vec<(String, usize)>, String> {
    let mut out = Vec::new();
    for name in jar.files.keys() {
        if !name.starts_with("net/minecraft/world/item/")
            || !name.ends_with("Item")
            || name.contains('$')
        {
            continue;
        }
        let Some(parsed) = jar.class(name) else {
            continue;
        };
        let mut extra = 0usize;
        for field in &parsed.fields {
            if field.access & 0x0008 != 0 {
                continue;
            }
            // `BlockItem.block` is the base field every block item carries; other
            // declared instance fields are what the exporter reports on top.
            if field.name == "block" {
                continue;
            }
            extra += 1;
        }
        if extra > 0 {
            out.push((name.rsplit('/').next().unwrap_or(name).to_string(), extra));
        }
    }
    out.sort();
    Ok(out)
}

// ---------------------------------------------------------------------------
// Class loading
// ---------------------------------------------------------------------------

pub struct Jar {
    files: BTreeMap<String, Vec<u8>>,
    parsed: RefCell<BTreeMap<String, Option<Rc<ClassFile>>>>,
}

impl Jar {
    pub fn open(path: &Path) -> Result<Jar, String> {
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

    /// Resolve a simple block class name through the vanilla `block` package tree.
    pub fn resolve(&self, simple: &str) -> Option<String> {
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

    pub fn class(&self, internal: &str) -> Option<Rc<ClassFile>> {
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
