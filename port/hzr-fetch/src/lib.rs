//! `hzr-fetch`: derive `assets/`, `data/` and `worldgen/` on the device from the
//! player's own Minecraft, so a route-3 transfer carries only the tables that
//! cannot be recomputed (see `route3/AUDIT.md`).
//!
//! Two inputs, both the player's own:
//!
//! 1. The **client jar** 閳?holds `assets/` (models, blockstates, textures, lang,
//!    shaders) and `data/` (recipes, loot tables, tags, advancements, worldgen).
//!    `worldgen/` in the prepared tree is a copy of `data/minecraft/worldgen/`.
//! 2. The **asset store** 閳?the indexed objects the jar only keeps placeholders
//!    for, or does not carry at all: sounds (the bulk), the six panorama faces,
//!    the Unifont glyphs and the non-English languages.
//!
//! The second one is why preparation needs `assets/indexes/<id>.json`: the jar's
//! panorama faces are 1x1 placeholders, and no `.ogg` lives in the jar.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub mod bundle;
pub mod facts;
pub mod gameplay;
pub mod index;
pub mod jar;
pub mod model_states;
pub mod nbt;
pub mod nbt_value;
pub mod version;
pub mod worldgen;

pub use index::{object_path, AssetIndex, Object};
pub use jar::{extract_jar, import_indexed, mirror_worldgen, Import, Stats};
pub use model_states::derive as derive_model_states;
pub use version::{AssetIndexRef, Download, VersionJson, VersionManifest, VersionRef};

/// Where a file came from.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Origin {
    /// Copied out of the client jar's `assets/` or `data/`.
    Jar,
    /// Copied from the asset store by its index hash.
    Store,
}

/// What one `extract` or `import` did.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Receipt {
    pub files: usize,
    pub bytes: u64,
    pub jar_files: usize,
    pub store_files: usize,
    /// Indexed names the store did not hold; the jar's version was used instead.
    pub fell_back_to_jar: Vec<String>,
    pub skipped: Vec<String>,
}

impl Receipt {
    pub fn add(&mut self, origin: Origin, size: u64) {
        self.files += 1;
        self.bytes += size;
        match origin {
            Origin::Jar => self.jar_files += 1,
            Origin::Store => self.store_files += 1,
        }
    }
}

/// `worldgen/` is a view of `data/minecraft/worldgen/`, not a separate download.
pub const WORLDGEN_PREFIX: &str = "data/minecraft/worldgen/";

/// Every file the prepared tree wants from the jar, relative and `/`-separated.
///
/// Mirrors `horizon-prepare`'s `extract_client_resources`: `assets/`, `data/`,
/// and the icon at the root.
pub fn wanted_from_jar(name: &str) -> bool {
    if name.ends_with('/') {
        return false;
    }
    name.starts_with("assets/") || name.starts_with("data/") || name == "pack.png"
}

/// The store backs up these subtrees; the jar's copies are placeholders.
pub fn indexed_prefix(name: &str) -> bool {
    name.starts_with("minecraft/sounds/")
        || name.starts_with("minecraft/font/")
        || name.starts_with("minecraft/lang/")
        || name.starts_with("minecraft/textures/gui/title/background/")
}

/// `<store>/<first two hash chars>/<hash>`, the asset store's layout.
pub fn store_path(store: &Path, hash: &str) -> PathBuf {
    object_path(store, hash)
}

/// Lowercase hex SHA-1, what an asset index and a version JSON check.
pub fn sha1_hex(data: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(data);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_jar_supplies_assets_data_and_the_icon_only() {
        assert!(wanted_from_jar("assets/minecraft/pack.mcmeta"));
        assert!(wanted_from_jar("data/minecraft/recipe/stick.json"));
        assert!(wanted_from_jar("pack.png"));
        assert!(!wanted_from_jar("META-INF/MANIFEST.MF"));
        assert!(!wanted_from_jar("net/minecraft/client/Minecraft.class"));
        assert!(!wanted_from_jar("assets/"));
    }

    #[test]
    fn the_store_backs_sounds_fonts_languages_and_the_panorama() {
        for name in [
            "minecraft/sounds/music/game/calm1.ogg",
            "minecraft/font/unifont.zip",
            "minecraft/lang/de_de.json",
            "minecraft/textures/gui/title/background/panorama_0.png",
        ] {
            assert!(indexed_prefix(name), "{name}");
        }
        assert!(!indexed_prefix("minecraft/textures/block/stone.png"));
    }

    #[test]
    fn store_paths_are_the_first_two_hash_chars_then_the_hash() {
        let hash = "ab12cd34ef567890ab12cd34ef567890ab12cd34";
        assert_eq!(
            store_path(Path::new("store"), hash),
            Path::new("store/ab").join(hash)
        );
    }

    #[test]
    fn sha1_matches_a_known_value() {
        // `printf '' | sha1sum`
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        // `printf 'abc' | sha1sum`
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }
}
