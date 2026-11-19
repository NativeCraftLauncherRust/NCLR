//! `hzr-download`: fetch the player's own Minecraft files from Mojang, on device.
//!
//! This closes the loop route 3 opened. `hzr-fetch` derives `assets/`, `data/`
//! and `worldgen/` from a client jar and an asset store; this crate gets the jar
//! and the store from Mojang, verified by the SHA-1 the version JSON and the asset
//! index publish. Nothing here needs a desktop, and nothing here carries a game
//! file: the bytes travel from Mojang's CDN to the licensee.
//!
//! Ownership is the caller's business. This crate downloads; it does not decide
//! who may download, and it holds no credentials.

pub mod auth;
pub mod callback;
pub mod fetch;
pub mod plan;

pub use fetch::{download, Report, Transport};
#[cfg(feature = "http")]
pub use fetch::HttpTransport;
pub use plan::{
    missing, missing_verified, object_files, root_files, Plan, RemoteFile, VERSION_MANIFEST,
};

use hzr_fetch::{VersionJson, VersionManifest, VersionRef};

/// Where Minecraft reports what the account owns.
pub const ENTITLEMENTS: &str = "https://api.minecraftservices.com/entitlements/mcstore";

/// Whether `access_token` belongs to an account that owns Java Edition.
///
/// The download itself needs no account; this is the check that makes a first run
/// legitimate. A token without the game, or an expired one, answers false rather
/// than carrying on.
pub fn owns_game<T: Transport>(transport: &T, access_token: &str) -> Result<bool, String> {
    let bytes = transport.fetch_authorized(ENTITLEMENTS, access_token)?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("entitlements: {e}"))?;
    let items = value
        .get("items")
        .and_then(serde_json::Value::as_array)
        .ok_or("entitlements: no items")?;
    Ok(items.iter().any(|item| {
        item.get("name")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|name| name == "product_minecraft")
    }))
}

/// The row `version` names in the manifest, or the latest release for `latest`.
pub fn resolve<T: Transport>(transport: &T, version: &str) -> Result<VersionRef, String> {
    let bytes = transport.fetch(VERSION_MANIFEST)?;
    let manifest = VersionManifest::parse(&bytes)?;
    if version == "latest" {
        return manifest
            .latest_release()
            .cloned()
            .ok_or_else(|| format!("the manifest has no release {}", manifest.latest.release));
    }
    manifest
        .version(version)
        .cloned()
        .ok_or_else(|| format!("the manifest has no version {version}"))
}

/// The version JSON `reference` points at.
pub fn version_json<T: Transport>(
    transport: &T,
    reference: &VersionRef,
) -> Result<VersionJson, String> {
    let bytes = transport.fetch(&reference.url)?;
    VersionJson::parse(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const MANIFEST: &str = r#"{
        "latest": {"release": "26.3", "snapshot": "26.4-snapshot"},
        "versions": [
            {"id": "26.4-snapshot", "type": "snapshot", "url": "https://example.invalid/s.json"},
            {"id": "26.3", "type": "release", "url": "https://example.invalid/26.3.json"}
        ]
    }"#;

    const VERSION: &str = r#"{
        "id": "26.3",
        "downloads": {"client": {"sha1": "aa", "size": 1, "url": "https://example.invalid/c.jar"}},
        "assetIndex": {"id": "34", "sha1": "bb", "size": 1, "url": "https://example.invalid/34.json"}
    }"#;

    struct Fake(BTreeMap<String, Vec<u8>>);

    impl Transport for Fake {
        fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
            self.0
                .get(url)
                .cloned()
                .ok_or_else(|| format!("{url}: not in the fixture"))
        }
    }

    fn fixture() -> Fake {
        Fake(BTreeMap::from([
            (VERSION_MANIFEST.to_string(), MANIFEST.as_bytes().to_vec()),
            (
                "https://example.invalid/26.3.json".to_string(),
                VERSION.as_bytes().to_vec(),
            ),
        ]))
    }

    #[test]
    fn a_named_version_resolves_to_its_row() {
        let reference = resolve(&fixture(), "26.3").unwrap();
        assert_eq!(reference.id, "26.3");
        assert_eq!(reference.url, "https://example.invalid/26.3.json");
    }

    #[test]
    fn latest_resolves_to_the_manifests_release() {
        let reference = resolve(&fixture(), "latest").unwrap();
        assert_eq!(reference.id, "26.3");
    }

    #[test]
    fn an_unknown_version_names_what_was_asked_for() {
        let error = resolve(&fixture(), "27.0").unwrap_err();
        assert!(error.contains("27.0"), "{error}");
    }

    #[test]
    fn the_version_json_comes_from_the_row_it_resolved() {
        let transport = fixture();
        let reference = resolve(&transport, "26.3").unwrap();
        let version = version_json(&transport, &reference).unwrap();
        assert_eq!(version.id, "26.3");
        assert_eq!(version.asset_index.id, "34");
    }
}

#[cfg(test)]
mod entitlement_tests {
    use super::*;
    use std::collections::BTreeMap;

    struct Fake {
        body: Vec<u8>,
        /// The token the fixture expects; anything else is a 401.
        expects: &'static str,
        seen: std::cell::RefCell<Option<String>>,
    }

    impl Transport for Fake {
        fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
            Err(format!("{url}: the entitlement check must use the token"))
        }
        fn fetch_authorized(&self, _url: &str, token: &str) -> Result<Vec<u8>, String> {
            *self.seen.borrow_mut() = Some(token.to_string());
            if token != self.expects {
                return Err("401 Unauthorized".into());
            }
            Ok(self.body.clone())
        }
    }

    fn fake(owned: bool, expects: &'static str) -> Fake {
        let body = if owned {
            r#"{"items":[{"name":"product_minecraft","signature":"x"}],"signature":"y"}"#
        } else {
            r#"{"items":[{"name":"product_dungeons","signature":"x"}],"signature":"y"}"#
        };
        Fake {
            body: body.as_bytes().to_vec(),
            expects,
            seen: std::cell::RefCell::new(None),
        }
    }

    #[test]
    fn an_account_that_owns_the_game_passes() {
        let transport = fake(true, "token-a");
        assert!(owns_game(&transport, "token-a").unwrap());
        assert_eq!(transport.seen.borrow().as_deref(), Some("token-a"));
    }

    #[test]
    fn an_account_without_it_does_not() {
        let transport = fake(false, "token-b");
        assert!(!owns_game(&transport, "token-b").unwrap());
    }

    #[test]
    fn an_expired_token_is_an_error_not_a_pass() {
        let transport = fake(true, "good");
        let error = owns_game(&transport, "stale").unwrap_err();
        assert!(error.contains("401"), "{error}");
    }

    #[test]
    fn a_body_without_items_is_refused() {
        struct Empty;
        impl Transport for Empty {
            fn fetch(&self, _: &str) -> Result<Vec<u8>, String> {
                Ok(b"{}".to_vec())
            }
            fn fetch_authorized(&self, _: &str, _: &str) -> Result<Vec<u8>, String> {
                Ok(b"{}".to_vec())
            }
        }
        let _ = BTreeMap::<u8, u8>::new();
        assert!(owns_game(&Empty, "t").is_err());
    }
}