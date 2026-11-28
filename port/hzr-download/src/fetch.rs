//! Fetching a plan, verified byte for byte.
//!
//! The transport is a trait so the whole loop is tested against canned bytes.
//! Verification happens **before** a file is written, so a truncated or
//! tampered response never lands in the player's store.

use crate::plan::{Plan, RemoteFile};
use std::fs;
#[cfg(feature = "http")]
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
#[cfg(feature = "http")]
use std::time::Duration;

/// Where bytes come from.
pub trait Transport {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, String>;

    /// Fetch with a bearer token. The default ignores it, which is enough for
    /// the fixtures; a real transport overrides this.
    fn fetch_authorized(&self, url: &str, _token: &str) -> Result<Vec<u8>, String> {
        self.fetch(url)
    }

    /// POST a body. The default refuses, so a fixture that only reads still
    /// compiles and a surprising POST fails loudly.
    fn post(&self, url: &str, _content_type: &str, _body: &str) -> Result<Vec<u8>, String> {
        Err(format!("{url}: this transport cannot POST"))
    }
}

/// HTTPS over the system trust store, through `ureq`/`rustls`.
///
/// Only built with the `http` feature. The iOS entry point uses this transport
/// with the system trust store and the agent's connection pool.
#[cfg(feature = "http")]
pub struct HttpTransport {
    agent: ureq::Agent,
}

#[cfg(feature = "http")]
impl HttpTransport {
    pub fn new() -> HttpTransport {
        HttpTransport {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(20))
                // A slow but progressing CDN response should be allowed to
                // finish. The read timeout only fires when no bytes arrive.
                .timeout_read(Duration::from_secs(45))
                .timeout_write(Duration::from_secs(45))
                // Keep enough idle sockets to reuse them across a wide asset
                // download. Every object comes from one CDN host, so the
                // per-host pool has to reach the concurrency ceiling (64).
                .max_idle_connections(128)
                .max_idle_connections_per_host(64)
                .build(),
        }
    }
}

#[cfg(feature = "http")]
impl Default for HttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "http")]
impl HttpTransport {
    fn get(&self, url: &str, token: Option<&str>) -> Result<Vec<u8>, String> {
        const MAX_ATTEMPTS: usize = 3;
        let mut failure = String::from("request failed");

        for attempt in 0..MAX_ATTEMPTS {
            let mut request = self.agent.get(url);
            if let Some(token) = token {
                request = request.set("Authorization", &format!("Bearer {token}"));
            }
            match request.call() {
                Ok(response) => {
                    let mut bytes = Vec::new();
                    match response.into_reader().read_to_end(&mut bytes) {
                        Ok(_) => return Ok(bytes),
                        Err(error) => failure = error.to_string(),
                    }
                }
                Err(ureq::Error::Status(status, _)) => {
                    return Err(format!("{url}: HTTP {status}"));
                }
                Err(error) => failure = error.to_string(),
            }

            if attempt + 1 < MAX_ATTEMPTS {
                std::thread::sleep(Duration::from_millis(400 * (attempt as u64 + 1)));
            }
        }

        Err(format!("{url}: {failure}"))
    }
}

#[cfg(feature = "http")]
impl Transport for HttpTransport {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
        self.get(url, None)
    }
    fn fetch_authorized(&self, url: &str, token: &str) -> Result<Vec<u8>, String> {
        self.get(url, Some(token))
    }
    fn post(&self, url: &str, content_type: &str, body: &str) -> Result<Vec<u8>, String> {
        let response = self
            .agent
            .post(url)
            .set("Content-Type", content_type)
            .send_string(body)
            .map_err(|error| format!("{url}: {error}"))?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .map_err(|error| format!("{url}: {error}"))?;
        Ok(bytes)
    }
}

/// What one run did.
#[derive(Debug, Default)]
pub struct Report {
    pub downloaded: usize,
    pub skipped: usize,
    pub bytes: u64,
    /// `(path, why)` for every file that did not arrive.
    pub failures: Vec<(String, String)>,
}

impl Report {
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Whether `path` already holds exactly what `file` describes.
fn satisfied(file: &RemoteFile) -> bool {
    match fs::read(file.path()) {
        Ok(bytes) => bytes.len() as u64 == file.size && hzr_fetch::sha1_hex(&bytes) == file.sha1,
        Err(_) => false,
    }
}

impl RemoteFile {
    fn path(&self) -> &Path {
        &self.path
    }
}

/// Write `bytes` at `file`'s path through a temporary file, so an interrupted
/// run leaves no half file behind.
fn store(file: &RemoteFile, bytes: &[u8]) -> Result<(), String> {
    let path = file.path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let temporary = path.with_extension("part");
    {
        let mut handle =
            fs::File::create(&temporary).map_err(|e| format!("{}: {e}", temporary.display()))?;
        handle
            .write_all(bytes)
            .map_err(|e| format!("{}: {e}", temporary.display()))?;
        handle
            .flush()
            .map_err(|e| format!("{}: {e}", temporary.display()))?;
    }
    fs::rename(&temporary, path).map_err(|e| format!("{}: {e}", path.display()))
}

fn fetch_one<T: Transport>(transport: &T, file: &RemoteFile) -> Result<u64, String> {
    let bytes = transport.fetch(&file.url)?;
    if bytes.len() as u64 != file.size {
        return Err(format!(
            "expected {} bytes, got {}",
            file.size,
            bytes.len()
        ));
    }
    let hash = hzr_fetch::sha1_hex(&bytes);
    if hash != file.sha1 {
        return Err(format!(
            "sha1 {hash}, the version publishes {}",
            file.sha1
        ));
    }
    store(file, &bytes)?;
    Ok(bytes.len() as u64)
}

/// Fetch every file `plan` names, skipping the ones already correct.
///
/// `progress` is called after each file with the file and its byte count.
pub fn download<T: Transport>(
    transport: &T,
    plan: &Plan,
    mut progress: impl FnMut(&RemoteFile, u64),
) -> Report {
    let mut report = Report::default();
    for file in &plan.files {
        if satisfied(file) {
            report.skipped += 1;
            progress(file, file.size);
            continue;
        }
        match fetch_one(transport, file) {
            Ok(bytes) => {
                report.downloaded += 1;
                report.bytes += bytes;
                progress(file, bytes);
            }
            Err(error) => report.failures.push((show(file), error)),
        }
    }
    report
}

/// Fetch every missing file with up to `concurrency` workers.
///
/// Verification and atomic writes still happen in each worker. The progress
/// callback runs on the caller's thread as results arrive, so callers do not
/// need to make their UI or logging callback thread-safe.
pub fn download_with_concurrency<T: Transport + Sync>(
    transport: &T,
    plan: &Plan,
    concurrency: usize,
    mut progress: impl FnMut(&RemoteFile, u64),
) -> Report {
    let mut report = Report::default();
    let mut pending = Vec::new();

    for (index, file) in plan.files.iter().enumerate() {
        if satisfied(file) {
            report.skipped += 1;
            progress(file, file.size);
        } else {
            pending.push(index);
        }
    }

    if pending.is_empty() {
        return report;
    }

    let worker_count = concurrency.max(1).min(pending.len());
    let pending = pending.as_slice();
    let next = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::channel();

    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            let next = Arc::clone(&next);
            let sender = sender.clone();
            scope.spawn(move || loop {
                let position = next.fetch_add(1, Ordering::Relaxed);
                let Some(&index) = pending.get(position) else {
                    break;
                };
                let result = fetch_one(transport, &plan.files[index]);
                sender.send((index, result)).expect("download receiver is alive");
            });
        }
        drop(sender);

        for (index, result) in receiver {
            let file = &plan.files[index];
            match result {
                Ok(bytes) => {
                    report.downloaded += 1;
                    report.bytes += bytes;
                    progress(file, bytes);
                }
                Err(error) => report.failures.push((show(file), error)),
            }
        }
    });

    report.failures.sort_by(|left, right| left.0.cmp(&right.0));
    report
}

fn show(file: &RemoteFile) -> String {
    format!("{} ({})", file.label, file.path.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    struct Fake(BTreeMap<String, Vec<u8>>);

    impl Transport for Fake {
        fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
            self.0
                .get(url)
                .cloned()
                .ok_or_else(|| format!("{url}: not in the fixture"))
        }
    }

    struct ConcurrentFake {
        files: BTreeMap<String, Vec<u8>>,
        active: Arc<AtomicUsize>,
        maximum: Arc<AtomicUsize>,
    }

    impl Transport for ConcurrentFake {
        fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.maximum.fetch_max(active, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(20));
            let result = self
                .files
                .get(url)
                .cloned()
                .ok_or_else(|| format!("{url}: not in the fixture"));
            self.active.fetch_sub(1, Ordering::SeqCst);
            result
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hzr-download-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file(root: &Path, name: &str, body: &[u8]) -> RemoteFile {
        RemoteFile {
            url: format!("https://example.invalid/{name}"),
            path: root.join(name),
            sha1: hzr_fetch::sha1_hex(body),
            size: body.len() as u64,
            label: name.into(),
        }
    }

    #[test]
    fn a_plan_downloads_verifies_and_then_skips() {
        let root = scratch("ok");
        let (a, b) = (b"alpha".as_slice(), b"beta-beta".as_slice());
        let plan = Plan {
            files: vec![file(&root, "a.bin", a), file(&root, "sub/b.bin", b)],
        };
        let transport = Fake(BTreeMap::from([
            (plan.files[0].url.clone(), a.to_vec()),
            (plan.files[1].url.clone(), b.to_vec()),
        ]));

        let report = download(&transport, &plan, |_, _| {});
        assert!(report.is_clean(), "{:?}", report.failures);
        assert_eq!(report.downloaded, 2);
        assert_eq!(report.bytes, 5 + 9);
        assert_eq!(fs::read(root.join("a.bin")).unwrap(), a);
        assert_eq!(fs::read(root.join("sub/b.bin")).unwrap(), b);

        // Second run: everything is already correct.
        let again = download(&transport, &plan, |_, _| {});
        assert_eq!(again.skipped, 2);
        assert_eq!(again.downloaded, 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_plan_can_download_with_multiple_workers() {
        let root = scratch("concurrent");
        let body = b"payload".as_slice();
        let files: Vec<RemoteFile> = (0..4)
            .map(|index| file(&root, &format!("{index}.bin"), body))
            .collect();
        let transport = ConcurrentFake {
            files: files
                .iter()
                .map(|file| (file.url.clone(), body.to_vec()))
                .collect(),
            active: Arc::new(AtomicUsize::new(0)),
            maximum: Arc::new(AtomicUsize::new(0)),
        };
        let maximum = Arc::clone(&transport.maximum);
        let report = download_with_concurrency(&transport, &Plan { files }, 4, |_, _| {});

        assert!(report.is_clean(), "{:?}", report.failures);
        assert_eq!(report.downloaded, 4);
        assert!(maximum.load(Ordering::SeqCst) >= 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_payload_that_does_not_match_its_hash_never_lands() {
        let root = scratch("corrupt");
        let expected = b"the real bytes";
        let plan = Plan {
            files: vec![file(&root, "a.bin", expected)],
        };
        let transport = Fake(BTreeMap::from([(
            plan.files[0].url.clone(),
            b"tampered------".to_vec(),
        )]));
        let report = download(&transport, &plan, |_, _| {});
        assert!(!report.is_clean());
        assert_eq!(report.downloaded, 0);
        assert!(report.failures[0].1.contains("sha1"));
        assert!(!root.join("a.bin").exists());
        assert!(!root.join("a.part").exists(), "a temporary file was left behind");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_short_payload_is_refused_by_size() {
        let root = scratch("short");
        let plan = Plan {
            files: vec![file(&root, "a.bin", b"1234567890")],
        };
        let transport = Fake(BTreeMap::from([(plan.files[0].url.clone(), b"123".to_vec())]));
        let report = download(&transport, &plan, |_, _| {});
        assert_eq!(report.downloaded, 0);
        assert!(report.failures[0].1.contains("expected 10 bytes"));
        assert!(!root.join("a.bin").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_endpoint_is_reported_and_the_rest_still_runs() {
        let root = scratch("partial");
        let body = b"ok".as_slice();
        let mut plan = Plan {
            files: vec![file(&root, "a.bin", body), file(&root, "b.bin", body)],
        };
        plan.files[1].url = "https://example.invalid/absent".into();
        let transport = Fake(BTreeMap::from([(plan.files[0].url.clone(), body.to_vec())]));
        let report = download(&transport, &plan, |_, _| {});
        assert_eq!(report.downloaded, 1);
        assert_eq!(report.failures.len(), 1);
        assert!(root.join("a.bin").exists());
        assert!(!root.join("b.bin").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_file_with_the_right_size_but_wrong_bytes_is_refetched() {
        let root = scratch("stale");
        fs::create_dir_all(&root).unwrap();
        let body = b"fresh".as_slice();
        let target = file(&root, "a.bin", body);
        fs::write(&target.path, b"stale").unwrap();
        let transport = Fake(BTreeMap::from([(target.url.clone(), body.to_vec())]));
        let report = download(&transport, &Plan { files: vec![target.clone()] }, |_, _| {});
        assert_eq!(report.downloaded, 1);
        assert_eq!(fs::read(&target.path).unwrap(), body);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn a_truncated_file_is_fetched_again() {
        // What an interrupted download leaves behind: the file exists, one byte
        // short. Size alone catches it before the hash is even computed.
        let root = scratch("truncated");
        fs::create_dir_all(&root).unwrap();
        let body = b"complete payload".as_slice();
        let target = file(&root, "a.bin", body);
        fs::write(&target.path, &body[..body.len() - 1]).unwrap();
        let transport = Fake(BTreeMap::from([(target.url.clone(), body.to_vec())]));
        let report = download(&transport, &Plan { files: vec![target.clone()] }, |_, _| {});
        assert_eq!(report.downloaded, 1);
        assert_eq!(fs::read(&target.path).unwrap(), body);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_leftover_part_file_is_not_the_target() {
        // `store` writes through `<name>.part`; a run killed mid-write leaves one.
        // It must not make the target look present, and it must not be served.
        let root = scratch("part");
        fs::create_dir_all(&root).unwrap();
        let body = b"the real bytes".as_slice();
        let target = file(&root, "a.bin", body);
        let part = target.path.with_extension("part");
        fs::write(&part, b"half writ").unwrap();

        let transport = Fake(BTreeMap::from([(target.url.clone(), body.to_vec())]));
        let report = download(&transport, &Plan { files: vec![target.clone()] }, |_, _| {});
        assert_eq!(report.downloaded, 1, "{:?}", report.failures);
        assert_eq!(fs::read(&target.path).unwrap(), body);

        // A second run sees the finished file and downloads nothing.
        let report = download(&transport, &Plan { files: vec![target.clone()] }, |_, _| {});
        assert_eq!(report.skipped, 1);
        // The temporary name is reused rather than ignored, so nothing
        // accumulates: the successful write consumed it.
        assert!(!part.exists(), "the temporary file was left behind");
        let _ = fs::remove_dir_all(root);
    }
}
