//! Reading the client jar, and preferring the asset store where it is richer.

use crate::index::{object_path, AssetIndex};
use crate::{indexed_prefix, sha1_hex, wanted_from_jar, Origin, Receipt};
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

/// What one extraction or import did.
pub type Stats = Receipt;

/// The indexed subtrees a caller can import on their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Import {
    Sounds,
    Fonts,
    /// The language files, and the asset index's `pack.mcmeta` beside them.
    Languages,
    Panorama,
    /// Programmer Art and High Contrast: zips only the index carries.
    Packs,
}

impl Import {
    pub const ALL: [Import; 5] = [
        Import::Sounds,
        Import::Fonts,
        Import::Languages,
        Import::Panorama,
        Import::Packs,
    ];

    /// Whether `name` (without the `assets/` prefix) belongs to this subtree.
    pub fn wanted(self, name: &str) -> bool {
        match self {
            Import::Sounds => name.starts_with("minecraft/sounds/"),
            Import::Fonts => name.starts_with("minecraft/font/"),
            Import::Languages => {
                name.starts_with("minecraft/lang/") || name == "pack.mcmeta"
            }
            Import::Panorama => name.starts_with("minecraft/textures/gui/title/background/"),
            Import::Packs => name.starts_with("minecraft/resourcepacks/"),
        }
    }
}

fn safe_relative(name: &str) -> Option<&str> {
    if name.is_empty() || name.starts_with('/') {
        return None;
    }
    if name
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return None;
    }
    Some(name)
}

fn write_member(out: &Path, relative: &str, data: &[u8]) -> Result<(), String> {
    let path = out.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(&path, data).map_err(|e| format!("{}: {e}", path.display()))
}

/// Copy every `assets/`, `data/` and `pack.png` member of `jar` under `out`.
///
/// Where the jar holds a placeholder that the asset store has a real file for
/// (panorama faces, fonts, languages), the store's copy wins when `index` and
/// `store` are given. A store miss falls back to the jar and is reported.
pub fn extract_jar(
    jar: &Path,
    out: &Path,
    index: Option<&AssetIndex>,
    store: Option<&Path>,
) -> Result<Stats, String> {
    let file = File::open(jar).map_err(|e| format!("{}: {e}", jar.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("{}: not a readable jar: {e}", jar.display()))?;
    let mut receipt = Stats::default();
    let mut buffer = Vec::new();

    for position in 0..archive.len() {
        let mut member = archive
            .by_index(position)
            .map_err(|e| format!("{}: member {position}: {e}", jar.display()))?;
        if member.is_dir() {
            continue;
        }
        let name = member
            .name()
            .map_err(|e| format!("{}: member {position}: {e}", jar.display()))?
            .to_string();
        if !wanted_from_jar(&name) {
            continue;
        }
        let Some(relative) = safe_relative(&name) else {
            receipt.skipped.push(name);
            continue;
        };
        buffer.clear();
        member
            .read_to_end(&mut buffer)
            .map_err(|e| format!("{name}: {e}"))?;

        // The store's version of an indexed name is the real one; the jar keeps
        // a 1x1 placeholder for the panorama and no audio at all.
        let stripped = name.strip_prefix("assets/");
        if let (Some(stripped), Some(index), Some(store)) = (stripped, index, store) {
            if indexed_prefix(stripped) {
                if let Some(object) = index.object(stripped) {
                    let source = object_path(store, &object.hash);
                    match fs::read(&source) {
                        Ok(store_bytes) if sha1_hex(&store_bytes) == object.hash => {
                            write_member(out, relative, &store_bytes)?;
                            receipt.add(Origin::Store, store_bytes.len() as u64);
                            continue;
                        }
                        _ => receipt.fell_back_to_jar.push(name.clone()),
                    }
                }
            }
        }
        write_member(out, relative, &buffer)?;
        receipt.add(Origin::Jar, buffer.len() as u64);
    }
    Ok(receipt)
}

/// `worldgen/` in the prepared tree is a view of two jar trees.
///
/// `data/minecraft/worldgen/` lands directly under `worldgen/`, and the structure
/// templates in `data/minecraft/structure/` land under `worldgen/structure/minecraft/`.
/// The rest of `worldgen/` (`biomes.json`, `biome_parameters/`, `blocks/`, ...) is
/// data-generator output and is **not** derivable from the jar.
pub fn mirror_worldgen(out: &Path) -> Result<usize, String> {
    fn copy_tree(from: &Path, to: &Path, copied: &mut usize) -> Result<(), String> {
        for entry in fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
            let entry = entry.map_err(|e| format!("{}: {e}", from.display()))?;
            let path = entry.path();
            let target = to.join(entry.file_name());
            let kind = entry
                .file_type()
                .map_err(|e| format!("{}: {e}", path.display()))?;
            if kind.is_dir() {
                fs::create_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
                copy_tree(&path, &target, copied)?;
            } else if kind.is_file() {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| format!("{}: {e}", parent.display()))?;
                }
                fs::copy(&path, &target).map_err(|e| format!("{}: {e}", path.display()))?;
                *copied += 1;
            }
        }
        Ok(())
    }

    let data = out.join("data/minecraft");
    let mut copied = 0;
    let mut found = false;
    for (source, destination) in [
        ("worldgen", "worldgen"),
        ("structure", "worldgen/structure/minecraft"),
    ] {
        let from = data.join(source);
        if !from.is_dir() {
            continue;
        }
        found = true;
        let to = out.join(destination);
        fs::create_dir_all(&to).map_err(|e| format!("{}: {e}", to.display()))?;
        copy_tree(&from, &to, &mut copied)?;
    }
    if !found {
        return Err(format!(
            "{}: the jar carried neither data/minecraft/worldgen nor .../structure",
            data.display()
        ));
    }
    Ok(copied)
}

/// Write the indexed files `imports` selects, verifying every one against its hash.
///
/// This is the 380 MB of sound the jar never carries, plus the panorama faces,
/// the Unifont glyphs and the languages other than English.
pub fn import_indexed(
    out: &Path,
    index: &AssetIndex,
    store: &Path,
    imports: &[Import],
) -> Result<Stats, String> {
    let mut receipt = Stats::default();
    for (name, object) in index.objects.iter() {
        if !imports.iter().any(|import| import.wanted(name)) {
            continue;
        }
        let source = object_path(store, &object.hash);
        let bytes = match fs::read(&source) {
            Ok(bytes) => bytes,
            Err(_) => {
                receipt.skipped.push(name.clone());
                continue;
            }
        };
        if object.size != 0 && bytes.len() as u64 != object.size {
            receipt.skipped.push(format!(
                "{name}: {} bytes, index says {}",
                bytes.len(),
                object.size
            ));
            continue;
        }
        if sha1_hex(&bytes) != object.hash {
            receipt.skipped.push(format!("{name}: sha1 does not match the index"));
            continue;
        }
        write_member(out, &format!("assets/{name}"), &bytes)?;
        receipt.add(Origin::Store, bytes.len() as u64);
    }
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hzr-fetch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn jar(path: &Path, members: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        for (name, data) in members {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }

    fn store_object(store: &Path, hash: &str, data: &[u8]) {
        let path = object_path(store, hash);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, data).unwrap();
    }

    #[test]
    fn only_assets_data_and_the_icon_leave_the_jar() {
        let root = scratch("jar");
        let archive = root.join("client.jar");
        jar(
            &archive,
            &[
                ("META-INF/MANIFEST.MF", b"manifest"),
                ("net/minecraft/client/Minecraft.class", b"bytecode"),
                ("assets/minecraft/pack.mcmeta", b"{\"pack\":{}}"),
                ("data/minecraft/recipe/stick.json", b"{}"),
                ("pack.png", b"png"),
            ],
        );
        let out = root.join("out");
        let receipt = extract_jar(&archive, &out, None, None).unwrap();
        assert_eq!(receipt.files, 3);
        assert_eq!(receipt.jar_files, 3);
        assert!(out.join("assets/minecraft/pack.mcmeta").is_file());
        assert!(out.join("data/minecraft/recipe/stick.json").is_file());
        assert!(out.join("pack.png").is_file());
        assert!(!out.join("net/minecraft/client/Minecraft.class").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_store_beats_the_jars_placeholder_and_a_miss_falls_back() {
        let root = scratch("prefer");
        let panorama = "minecraft/textures/gui/title/background/panorama_0.png";
        let missing = "minecraft/textures/gui/title/background/panorama_1.png";
        let real = vec![9u8; 1024];
        let archive = root.join("client.jar");
        jar(
            &archive,
            &[
                ("assets/minecraft/textures/gui/title/background/panorama_0.png", &[1, 1]),
                ("assets/minecraft/textures/gui/title/background/panorama_1.png", &[1, 1]),
            ],
        );
        // Face 0 is in the store; face 1's index hash is not, so the jar's
        // placeholder has to stand in and the fallback is reported.
        let mut objects = BTreeMap::new();
        objects.insert(
            panorama.to_string(),
            crate::index::Object {
                hash: sha1_hex(&real),
                size: real.len() as u64,
            },
        );
        objects.insert(
            missing.to_string(),
            crate::index::Object {
                hash: sha1_hex(b"never-written"),
                size: 13,
            },
        );
        let index = AssetIndex { objects };
        let store = root.join("store");
        store_object(&store, &sha1_hex(&real), &real);

        let out = root.join("out");
        let receipt = extract_jar(&archive, &out, Some(&index), Some(&store)).unwrap();
        assert_eq!(receipt.store_files, 1);
        assert_eq!(receipt.jar_files, 1);
        assert_eq!(receipt.fell_back_to_jar, vec![format!("assets/{missing}")]);
        assert_eq!(
            fs::read(out.join("assets/minecraft/textures/gui/title/background/panorama_0.png"))
                .unwrap()
                .len(),
            1024
        );
        assert_eq!(
            fs::read(out.join("assets/minecraft/textures/gui/title/background/panorama_1.png"))
                .unwrap(),
            vec![1, 1]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn importing_sounds_verifies_every_object() {
        let root = scratch("sounds");
        let good = b"OggS-good".to_vec();
        let bad = b"tampered".to_vec();
        let mut objects = BTreeMap::new();
        objects.insert(
            "minecraft/sounds/music/game/calm1.ogg".to_string(),
            crate::index::Object {
                hash: sha1_hex(&good),
                size: good.len() as u64,
            },
        );
        objects.insert(
            "minecraft/sounds/music/game/calm2.ogg".to_string(),
            crate::index::Object {
                hash: sha1_hex(b"original"),
                size: 8,
            },
        );
        let index = AssetIndex { objects };
        let store = root.join("store");
        store_object(&store, &sha1_hex(&good), &good);
        store_object(&store, &sha1_hex(b"original"), &bad);
        store_object(&store, &sha1_hex(b"absent-size"), b"");

        let out = root.join("out");
        let receipt = import_indexed(&out, &index, &store, &[Import::Sounds]).unwrap();
        assert_eq!(receipt.store_files, 1);
        assert_eq!(receipt.skipped.len(), 1);
        assert!(receipt.skipped[0].contains("calm2.ogg"));
        assert!(out.join("assets/minecraft/sounds/music/game/calm1.ogg").is_file());
        assert!(!out.join("assets/minecraft/sounds/music/game/calm2.ogg").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_member_that_climbs_out_of_the_output_is_refused() {
        let root = scratch("slip");
        let archive = root.join("evil.jar");
        // zip-slip: the member name escapes the output directory.
        jar(&archive, &[("assets/../../evil.txt", b"pwned")]);
        let out = root.join("out");
        let receipt = extract_jar(&archive, &out, None, None).unwrap();
        assert_eq!(receipt.files, 0);
        assert_eq!(receipt.skipped.len(), 1);
        assert!(!root.join("evil.txt").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn import_filters_cover_the_indexed_subtrees() {
        for import in Import::ALL {
            assert!(import.wanted(match import {
                Import::Sounds => "minecraft/sounds/a.ogg",
                Import::Fonts => "minecraft/font/unifont.zip",
                Import::Languages => "minecraft/lang/de_de.json",
                Import::Panorama => "minecraft/textures/gui/title/background/panorama_0.png",
                Import::Packs => "minecraft/resourcepacks/programmer_art.zip",
            }));
            assert!(!import.wanted("minecraft/textures/block/stone.png"));
        }
        // The language step also writes the index's root `pack.mcmeta`, which
        // `check.rs` treats as the marker that languages were installed.
        assert!(Import::Languages.wanted("pack.mcmeta"));
    }
}
