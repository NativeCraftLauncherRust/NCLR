//! What to download, computed before anything is fetched.
//!
//! Every entry carries the SHA-1 and size the version JSON or the asset index
//! publishes, so a download can be verified against Mojang's own numbers instead
//! of trusting the connection.

use hzr_fetch::{object_path, AssetIndex, VersionJson};
use std::path::{Path, PathBuf};

/// The endpoints a fetcher reads. Public: no account needed to download, only
/// to own the game.
pub const VERSION_MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

/// One file to fetch, and where it belongs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteFile {
    pub url: String,
    /// Absolute destination.
    pub path: PathBuf,
    /// Lowercase hex, as the version JSON or the index publishes it.
    pub sha1: String,
    pub size: u64,
    /// What this is, for progress lines: `client jar`, `asset index`, `asset`.
    pub label: String,
}

/// The whole download, in the order it must run: the version's own files first,
/// then the asset index's objects once the index has been read.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub files: Vec<RemoteFile>,
}

impl Plan {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// The client jar and its asset index: everything needed before the index can be
/// read.
pub fn root_files(version: &VersionJson, out: &Path, store: &Path) -> Plan {
    Plan {
        files: vec![
            RemoteFile {
                url: version.downloads.client.url.clone(),
                path: out.join("client.jar"),
                sha1: version.downloads.client.sha1.clone(),
                size: version.downloads.client.size,
                label: "client jar".into(),
            },
            RemoteFile {
                // Kept beside the prepared resources so `hzr-fetch import` can
                // read it without knowing the version the player installed.
                url: version.asset_index.url.clone(),
                path: store.join("indexes").join(format!("{}.json", version.asset_index.id)),
                sha1: version.asset_index.sha1.clone(),
                size: version.asset_index.size,
                label: "asset index".into(),
            },
        ],
    }
}

/// Every object the index names that `wanted` accepts, into
/// `<store>/objects/<xx>/<hash>`.
///
/// A first run only needs the indexed subtrees the assembly imports (fonts,
/// languages, the panorama, the built-in packs): about 100 MB rather than the
/// whole 465 MB store. Everything else comes out of the client jar.
pub fn object_files_where(
    index: &AssetIndex,
    store: &Path,
    wanted: impl Fn(&str) -> bool,
) -> Plan {
    let objects_dir = store.join("objects");
    let mut files: Vec<RemoteFile> = index
        .objects
        .iter()
        .filter(|(name, _)| wanted(name))
        .map(|(name, object)| RemoteFile {
            url: format!(
                "https://resources.download.minecraft.net/{}/{}",
                &object.hash[..2],
                object.hash
            ),
            path: object_path(&objects_dir, &object.hash),
            sha1: object.hash.clone(),
            size: object.size,
            label: format!("asset {name}"),
        })
        .collect();
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files.dedup_by(|left, right| left.path == right.path);
    Plan { files }
}

/// Every object the index names, into `<store>/objects/<xx>/<hash>`.
///
/// `store` is the assets folder, so the layout is Mojang's own and
/// `hzr-fetch import --store <store>/objects` reads it as is.
pub fn object_files(index: &AssetIndex, store: &Path) -> Plan {
    object_files_where(index, store, |_| true)
}
/// `plan` without the entries whose destination already holds the right bytes.
///
/// A file is skipped only when its size and SHA-1 match, so a half-written or
/// corrupted previous run is fetched again rather than trusted.
pub fn missing(plan: &Plan) -> Plan {
    Plan {
        files: plan
            .files
            .iter()
            .filter(|file| !std::fs::metadata(&file.path).is_ok_and(|meta| meta.len() == file.size))
            .cloned()
            .collect(),
    }
}

/// `plan` without entries already present and correct, checked by hash.
pub fn missing_verified(plan: &Plan) -> Result<Plan, String> {
    let mut files = Vec::new();
    for file in &plan.files {
        match std::fs::read(&file.path) {
            Ok(bytes) if bytes.len() as u64 == file.size && hzr_fetch::sha1_hex(&bytes) == file.sha1 => {
                continue
            }
            _ => files.push(file.clone()),
        }
    }
    Ok(Plan { files })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const VERSION: &str = r#"{
        "id": "26.3",
        "downloads": {"client": {"sha1": "aa11", "size": 10, "url": "https://example.invalid/client.jar"}},
        "assetIndex": {"id": "34", "sha1": "bb22", "size": 20, "url": "https://example.invalid/34.json"}
    }"#;

    fn root(name: &str) -> PathBuf {
        // A counter as well, so tests running in parallel never share a
        // directory and delete each other's fixtures.
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "hzr-download-plan-{name}-{unique}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn index() -> AssetIndex {
        let mut objects = BTreeMap::new();
        for (name, hash) in [
            ("minecraft/sounds/a.ogg", "ab12cd34ef567890ab12cd34ef567890ab12cd34"),
            ("minecraft/lang/de_de.json", "0011223344556677889900112233445566778899"),
        ] {
            objects.insert(
                name.to_string(),
                hzr_fetch::Object {
                    hash: hash.into(),
                    size: 5,
                },
            );
        }
        AssetIndex { objects }
    }

    #[test]
    fn the_root_plan_is_the_jar_and_the_index() {
        let dir = root("auto");
        let version = VersionJson::parse(VERSION.as_bytes()).unwrap();
        let plan = root_files(&version, &dir.join("out"), &dir.join("store"));
        assert_eq!(plan.files.len(), 2);
        assert_eq!(plan.total_bytes(), 30);
        assert_eq!(plan.files[0].label, "client jar");
        assert!(plan.files[0].path.ends_with("client.jar"));
        assert!(plan
            .files[1]
            .path
            .ends_with(Path::new("store").join("indexes").join("34.json")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn objects_land_under_their_hash_and_are_fetched_once() {
        let dir = root("objects");
        let plan = object_files(&index(), &dir);
        assert_eq!(plan.files.len(), 2);
        // Sorted by path, so look the entry up rather than assume a position.
        let sounds = plan
            .files
            .iter()
            .find(|file| file.label.contains("sounds/a.ogg"))
            .expect("the sound is in the plan");
        assert!(sounds
            .url
            .starts_with("https://resources.download.minecraft.net/ab/"));
        assert!(sounds.path.ends_with("objects/ab/ab12cd34ef567890ab12cd34ef567890ab12cd34"));
        assert_eq!(plan.total_bytes(), 10);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_of_the_right_size_is_skipped_without_reading_it() {
        let dir = root("auto");
        let target = dir.join("a.bin");
        std::fs::write(&target, vec![0u8; 4]).unwrap();
        let plan = Plan {
            files: vec![RemoteFile {
                url: "u".into(),
                path: target,
                sha1: "x".into(),
                size: 4,
                label: "a".into(),
            }],
        };
        assert!(missing(&plan).is_empty());
        // A different size is not skipped.
        let bigger = Plan {
            files: vec![RemoteFile {
                url: "u".into(),
                path: dir.join("a.bin"),
                sha1: "x".into(),
                size: 5,
                label: "a".into(),
            }],
        };
        assert_eq!(missing(&bigger).files.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn verification_is_by_hash_not_only_by_size() {
        let dir = root("auto");
        let target = dir.join("a.bin");
        std::fs::write(&target, b"abcd").unwrap();
        let good = hzr_fetch::sha1_hex(b"abcd");
        let ok = Plan {
            files: vec![RemoteFile {
                url: "u".into(),
                path: target.clone(),
                sha1: good,
                size: 4,
                label: "a".into(),
            }],
        };
        assert!(missing_verified(&ok).unwrap().is_empty());
        let wrong = Plan {
            files: vec![RemoteFile {
                url: "u".into(),
                path: target,
                sha1: "0".repeat(40),
                size: 4,
                label: "a".into(),
            }],
        };
        assert_eq!(missing_verified(&wrong).unwrap().files.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
