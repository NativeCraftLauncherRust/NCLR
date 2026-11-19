//! The two documents Mojang publishes for a version: the manifest that lists
//! every version, and one version's own JSON. Only the fields a fetcher needs.

use serde::{Deserialize, Serialize};

/// `piston-meta.mojang.com/mc/game/version_manifest_v2.json`.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct VersionManifest {
    pub latest: Latest,
    #[serde(default)]
    pub versions: Vec<VersionRef>,
}

/// The release and snapshot the launcher defaults to.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Latest {
    pub release: String,
    pub snapshot: String,
}

/// One row of the manifest.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct VersionRef {
    pub id: String,
    pub url: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub sha1: String,
    #[serde(rename = "releaseTime", default)]
    pub release_time: String,
}

/// One version's JSON: what to download and with which Java.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct VersionJson {
    pub id: String,
    pub downloads: Downloads,
    #[serde(rename = "assetIndex")]
    pub asset_index: AssetIndexRef,
    #[serde(rename = "javaVersion", default)]
    pub java_version: Option<JavaVersion>,
}

/// Every file a version publishes, of which the fetcher needs `client`.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Downloads {
    pub client: Download,
}

/// A file and the SHA-1 that must match it.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Download {
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

/// The asset index this version points at.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct AssetIndexRef {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    #[serde(rename = "totalSize", default)]
    pub total_size: u64,
    pub url: String,
}

/// The Java the version asks for.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct JavaVersion {
    #[serde(rename = "majorVersion")]
    pub major: u32,
}

impl VersionManifest {
    pub fn parse(bytes: &[u8]) -> Result<VersionManifest, String> {
        serde_json::from_slice(bytes).map_err(|e| format!("version manifest: {e}"))
    }

    /// The row for `id`, whatever its type.
    pub fn version(&self, id: &str) -> Option<&VersionRef> {
        self.versions.iter().find(|version| version.id == id)
    }

    /// The newest release, by the manifest's own order.
    pub fn latest_release(&self) -> Option<&VersionRef> {
        self.version(&self.latest.release)
    }
}

impl VersionJson {
    pub fn parse(bytes: &[u8]) -> Result<VersionJson, String> {
        serde_json::from_slice(bytes).map_err(|e| format!("version json: {e}"))
    }

    /// The Java major the version asks for, when it says.
    pub fn java_major(&self) -> Option<u32> {
        self.java_version.as_ref().map(|java| java.major)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"{
        "latest": {"release": "26.3", "snapshot": "26.4-snapshot"},
        "versions": [
            {"id": "26.4-snapshot", "type": "snapshot", "url": "https://example.invalid/snap.json", "releaseTime": "2026-11-01T00:00:00+00:00"},
            {"id": "26.3", "type": "release", "url": "https://example.invalid/26.3.json", "sha1": "aa", "releaseTime": "2026-10-01T00:00:00+00:00"}
        ]
    }"#;

    const VERSION: &str = r#"{
        "id": "26.3",
        "downloads": {"client": {"sha1": "e877b6a07acd633fb3bb475002175cec036e7b87", "size": 123, "url": "https://example.invalid/client.jar"}},
        "assetIndex": {"id": "34", "sha1": "abfaa525f923f807df8b4e4d29c1b5e3a104adbe", "size": 597235, "totalSize": 900, "url": "https://example.invalid/34.json"},
        "javaVersion": {"majorVersion": 25}
    }"#;

    #[test]
    fn the_manifest_finds_a_version_and_the_latest_release() {
        let manifest = VersionManifest::parse(MANIFEST.as_bytes()).unwrap();
        assert_eq!(manifest.versions.len(), 2);
        assert_eq!(manifest.version("26.3").unwrap().kind, "release");
        assert_eq!(manifest.latest_release().unwrap().id, "26.3");
        assert!(manifest.version("1.20").is_none());
    }

    #[test]
    fn a_version_json_yields_the_jar_the_index_and_the_java() {
        let version = VersionJson::parse(VERSION.as_bytes()).unwrap();
        assert_eq!(version.id, "26.3");
        assert_eq!(version.downloads.client.size, 123);
        assert_eq!(version.asset_index.id, "34");
        assert_eq!(version.asset_index.total_size, 900);
        assert_eq!(version.java_major(), Some(25));
    }

    #[test]
    fn a_version_without_java_says_so_rather_than_guessing() {
        let text = r#"{
            "id": "26.3",
            "downloads": {"client": {"sha1": "e877", "size": 1, "url": "https://example.invalid/c.jar"}},
            "assetIndex": {"id": "34", "sha1": "abfa", "size": 1, "url": "https://example.invalid/34.json"}
        }"#;
        assert_eq!(VersionJson::parse(text.as_bytes()).unwrap().java_major(), None);
    }
}
