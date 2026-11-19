//! The asset index (`assets/indexes/<id>.json`) and the store layout it names.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// `{"objects": {"minecraft/sounds/...": {"hash": "...", "size": N}}}`.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct AssetIndex {
    #[serde(default)]
    pub objects: BTreeMap<String, Object>,
}

/// One indexed asset.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Object {
    pub hash: String,
    #[serde(default)]
    pub size: u64,
}

impl AssetIndex {
    /// Parse an index, rejecting one without the `objects` map.
    pub fn parse(bytes: &[u8]) -> Result<AssetIndex, String> {
        let index: AssetIndex =
            serde_json::from_slice(bytes).map_err(|e| format!("asset index: {e}"))?;
        if index.objects.is_empty() {
            return Err("asset index has no objects".into());
        }
        Ok(index)
    }

    /// The object `name` names, if the index has it.
    pub fn object(&self, name: &str) -> Option<&Object> {
        self.objects.get(name)
    }

    /// Sum of every object's size: what the store holds once complete.
    pub fn total_size(&self) -> u64 {
        self.objects.values().map(|o| o.size).sum()
    }

    /// The names a filter keeps, in index order.
    pub fn matching(&self, wanted: impl Fn(&str) -> bool) -> Vec<(&str, &Object)> {
        self.objects
            .iter()
            .filter(|(name, _)| wanted(name))
            .map(|(name, object)| (name.as_str(), object))
            .collect()
    }
}

/// `<store>/<first two hash chars>/<hash>`, the asset store's layout.
///
/// A malformed hash yields a path that cannot exist, rather than an error the
/// caller has to thread through a directory walk.
pub fn object_path(store: &Path, hash: &str) -> PathBuf {
    let valid = hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit());
    if !valid {
        return store.join("invalid").join(hash);
    }
    store.join(&hash[..2]).join(hash)
}

/// Whether `hash` looks like a SHA-1 the store could name.
pub fn is_hash(hash: &str) -> bool {
    hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = r#"{
        "objects": {
            "minecraft/sounds/music/game/calm1.ogg": {"hash": "ab12cd34ef567890ab12cd34ef567890ab12cd34", "size": 3},
            "minecraft/lang/de_de.json": {"hash": "0011223344556677889900112233445566778899", "size": 2}
        }
    }"#;

    #[test]
    fn an_index_parses_and_counts_its_objects() {
        let index = AssetIndex::parse(INDEX.as_bytes()).unwrap();
        assert_eq!(index.objects.len(), 2);
        assert_eq!(index.total_size(), 5);
        assert_eq!(
            index.object("minecraft/lang/de_de.json").unwrap().size,
            2
        );
        let sounds = index.matching(|name| name.contains("/sounds/"));
        assert_eq!(sounds.len(), 1);
    }

    #[test]
    fn an_index_without_objects_is_refused() {
        assert!(AssetIndex::parse(b"{}").is_err());
        assert!(AssetIndex::parse(b"not json").is_err());
    }

    #[test]
    fn object_paths_follow_the_store_layout() {
        let hash = "ab12cd34ef567890ab12cd34ef567890ab12cd34";
        assert_eq!(
            object_path(Path::new("store"), hash),
            Path::new("store/ab").join(hash)
        );
        // Anything that is not a SHA-1 goes somewhere that cannot collide.
        assert_eq!(
            object_path(Path::new("store"), "../../etc/passwd"),
            Path::new("store/invalid/../../etc/passwd")
        );
        assert!(!is_hash("abc"));
        assert!(is_hash(hash));
    }
}
