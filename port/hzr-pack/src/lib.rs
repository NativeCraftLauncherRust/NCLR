//! `hzr-pack`: one verifiable archive for prepared Horizon resources.
//!
//! Route 3 (see `route3/AUDIT.md`) concluded that 90.5 MB of prepared tables are
//! boot-critical and cannot be dropped, so the win is making that set *one file*
//! that can be versioned, transferred and checked as a unit - rather than a
//! 23000-file directory tree that fails halfway through an import.
//!
//! # Two independent axes
//!
//! A file's *need* and its *origin* are separate questions, and conflating them
//! hides the only number route 3 cares about:
//!
//! - [`Bucket`] - how badly the game needs it: fatal at boot (`A`), degrades
//!   instead of failing (`B`), or regenerated at runtime (`D`).
//! - [`Entry::computable`] - whether the user's own `client.jar` and asset store
//!   derive it on device, so it need not be transferred at all.
//!
//! `assets/`, `data/` and `worldgen/` are required **and** computable; `facts.json`
//! is required and must be packed. The manifest records both, so a launcher can
//! say "transfer 90.5 MB" instead of "transfer 238 MB".
//!
//! # Format (`HZR3` v1)
//!
//! ```text
//! offset 0     magic     8 bytes   "HZR3v1\0\0"
//! offset 8     length    8 bytes   u64 LE, the manifest's byte length
//! offset 16    manifest  length    UTF-8 JSON (`Manifest`)
//! ...          padding   zeros up to `blob_base`
//! blob_base    blobs     ...       `Manifest::entries` in order, each at its own offset
//! ```
//!
//! `blob_base` is `align_up(16 + length, 64)`, and each blob starts on an 8-byte
//! boundary so a reader can map it without copying. Offsets are relative to
//! `blob_base`, so the manifest's own length never feeds back into them.
//!
//! Every entry carries the SHA-256 of the bytes it points at: a truncated or
//! corrupted archive is caught before the game reads a single table.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Magic bytes: format name and version. Changing the layout means a new magic.
pub const MAGIC: [u8; 8] = *b"HZR3v1\0\0";
/// Magic plus the manifest length word.
pub const HEADER_LEN: u64 = 16;
/// The blob section starts on this boundary.
pub const ALIGN: u64 = 64;
/// What this crate writes into `Manifest::format`.
pub const FORMAT: u32 = 1;
/// A manifest longer than this is not one of ours; refuse it before allocating.
const MAX_MANIFEST: u64 = 64 << 20;
/// Blobs start on this boundary, so a reader can map one without copying.
const BLOB_ALIGN: u64 = 8;

/// How badly the game needs a file, from `route3/AUDIT.md`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bucket {
    /// Fatal at boot: the load path uses `?`.
    #[serde(rename = "A")]
    BootCritical,
    /// The load path degrades instead of failing.
    #[serde(rename = "B")]
    Degradable,
    /// Regenerated at runtime, or a log: never shipped.
    #[serde(rename = "D")]
    Droppable,
    /// Not covered by the audit; shipped as-is.
    #[serde(rename = "?")]
    Unknown,
}

impl Bucket {
    /// The single letter the manifest and the CLI print.
    pub fn letter(self) -> &'static str {
        match self {
            Bucket::BootCritical => "A",
            Bucket::Degradable => "B",
            Bucket::Droppable => "D",
            Bucket::Unknown => "?",
        }
    }
}

/// One file inside the blobs section.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Entry {
    /// Path relative to the packed directory, `/`-separated.
    pub path: String,
    pub bucket: Bucket,
    /// The user's own jar derives it; the pack need not carry it to a device.
    #[serde(default)]
    pub computable: bool,
    /// Content bytes, before compression.
    pub size: u64,
    /// Lowercase hex of the content.
    pub sha256: String,
    /// Relative to the blob section's start.
    pub offset: u64,
    /// Bytes actually written at `offset`.
    #[serde(default)]
    pub stored: u64,
    /// Whether those bytes are a raw deflate stream of the content.
    #[serde(default)]
    pub compressed: bool,
}

/// The archive's table of contents.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Manifest {
    pub format: u32,
    pub minecraft: String,
    pub output_version: u32,
    pub schema: u32,
    pub created_unix: u64,
    /// Bytes of file content, without the padding between blobs.
    pub total_bytes: u64,
    /// The blob section's length: `total_bytes` plus that padding.
    #[serde(default)]
    pub blob_bytes: u64,
    /// Files the device regenerates, which this archive deliberately leaves out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub omitted: Vec<String>,
    pub entries: Vec<Entry>,
}

/// What to record when packing.
#[derive(Clone, Debug)]
pub struct PackOptions {
    pub minecraft: String,
    pub output_version: u32,
    pub schema: u32,
    /// Keep `*.log`, `horizon-sprites.cache` and the prepare stamp too.
    pub include_droppable: bool,
    /// Deflate each blob when that makes it smaller.
    pub compress: bool,
    /// Leave out the files a device derives from the player's own jar, so the
    /// archive is exactly what has to travel.
    pub only_packed: bool,
    /// Paths the device regenerates, so they are not carried at all.
    ///
    /// `model-states.json` is one: `hzr-fetch model-states` rebuilds it from
    /// `facts.json` plus the jar's blockstates, and the result is identical
    /// (measured in `route3/AUDIT.md`).
    pub omit: Vec<String>,
}

impl Default for PackOptions {
    fn default() -> Self {
        PackOptions {
            minecraft: String::from("unknown"),
            output_version: 0,
            schema: 0,
            include_droppable: false,
            compress: true,
            only_packed: false,
            omit: Vec::new(),
        }
    }
}

/// What `verify` counted.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub files: usize,
    pub bytes: u64,
    pub per_bucket: BTreeMap<&'static str, usize>,
    /// Bytes of files the device can derive itself.
    pub computable_bytes: u64,
    /// Bytes a device must actually receive.
    pub packed_bytes: u64,
    pub missing_minecraft: bool,
}

fn align_up(value: u64, align: u64) -> u64 {
    debug_assert!(align.is_power_of_two());
    (value + align - 1) & !(align - 1)
}

/// Lowercase hex SHA-256 of a whole file, streamed.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether the user's own `client.jar` and asset store produce `path`, so a
/// device needs no copy of it from the pack.
///
/// Measured against a real prepare (`route3/AUDIT.md`): the jar reproduces every
/// `assets/` and `data/` file byte for byte, and `worldgen/` - **except** the
/// data-generator tables and the block-behaviour dumps, which are Java output.
pub fn is_computable(path: &str) -> bool {
    if path.starts_with("worldgen/") {
        let rest = &path["worldgen/".len()..];
        // A direct child file is a data-generator table (`biomes.json`,
        // `noises.json`, ...); a subfolder is a copy of the jar's data tree.
        let Some((directory, _)) = rest.split_once('/') else {
            return false;
        };
        return !matches!(directory, "blocks" | "biome_parameters");
    }
    path.starts_with("assets/")
        || path.starts_with("data/")
        || matches!(
            path,
            "asset-index.json" | "pack.png"
        )
}

/// How badly the game needs `path`, per `route3/AUDIT.md`.
///
/// Kept as one function so the audit and the tool cannot drift apart.
pub fn classify(path: &str) -> Bucket {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name.ends_with(".log")
        || name == "horizon-sprites.cache"
        || name == ".horizon-prepare.lock"
    {
        return Bucket::Droppable;
    }
    // Degradable: read with `.ok()`, as an `Option`, or behind an `exists`
    // check (AUDIT.md, bucket B).
    if matches!(
        name,
        "entity-models.json"
            | "mob-data.json"
            | "creative-inventory.json"
            | "datafix-tables.json"
    ) || name.starts_with("damage-")
    {
        return Bucket::Degradable;
    }
    // Boot-critical: AUDIT.md bucket A. `assets/`, `data/` and `worldgen/` are
    // required too, but they are also computable, so they land here as well.
    if matches!(
        name,
        "facts.json"
            | "gameplay-facts.json"
            | "block-behaviours.json"
            | "model-states.json"
            | "occlusion.nbt"
            | "cached-collision.nbt"
            | "movement-properties.nbt"
            | "render-definitions.json"
            | "attributes.json"
            | "landing-definitions.json"
            | "items.json"
            | "entity-types.json"
            | "command-registries.json"
            | "commands.json"
            | "registries.json"
            | "asset-index.json"
            | "block-tags.json"
            | "pack.png"
            | "player-float-ids.json"
    ) || path.starts_with("biome-parameters/")
        || path.starts_with("assets/")
        || path.starts_with("data/")
        || path.starts_with("worldgen/")
    {
        return Bucket::BootCritical;
    }
    Bucket::Unknown
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if kind.is_dir() {
            collect(root, &path, out)?;
        } else if kind.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| format!("{}: outside {}", path.display(), root.display()))?;
            let text = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push(text);
        }
    }
    Ok(())
}

/// Raw deflate of `bytes` at the best setting.
fn deflate(bytes: &[u8]) -> Result<Vec<u8>, String> {
    use flate2::write::DeflateEncoder;
    use std::io::Write;
    let mut encoder = DeflateEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(bytes).map_err(|e| e.to_string())?;
    encoder.finish().map_err(|e| e.to_string())
}

/// The inverse of [`deflate`].
fn inflate(bytes: &[u8], expected: u64) -> Result<Vec<u8>, String> {
    use flate2::write::DeflateDecoder;
    use std::io::Write;
    let mut decoder = DeflateDecoder::new(Vec::with_capacity(expected as usize));
    decoder.write_all(bytes).map_err(|e| e.to_string())?;
    decoder.finish().map_err(|e| e.to_string())
}

/// Lowercase hex SHA-256 of a byte slice.
pub fn sha256_of(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Pack `dir` into `out`. Entries are sorted by path, so two runs over the same
/// directory produce the same archive.
pub fn pack(dir: &Path, out: &Path, options: &PackOptions) -> Result<Manifest, String> {
    let mut paths = Vec::new();
    collect(dir, dir, &mut paths)?;
    paths.sort();

    let mut entries = Vec::with_capacity(paths.len());
    let mut blobs: Vec<Vec<u8>> = Vec::with_capacity(paths.len());
    let mut cursor = 0u64;
    let mut omitted = Vec::new();
    for relative in paths {
        if options.omit.iter().any(|skip| skip == &relative) {
            omitted.push(relative);
            continue;
        }
        let bucket = classify(&relative);
        if bucket == Bucket::Droppable && !options.include_droppable {
            continue;
        }
        if options.only_packed && is_computable(&relative) {
            continue;
        }
        let full = dir.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        let bytes = fs::read(&full).map_err(|e| format!("{}: {e}", full.display()))?;
        let size = bytes.len() as u64;
        if size == 0 && bucket == Bucket::Droppable {
            continue;
        }
        let sha256 = sha256_of(&bytes);
        let (blob, compressed) = if options.compress {
            let deflated = deflate(&bytes)?;
            if deflated.len() < bytes.len() {
                (deflated, true)
            } else {
                (bytes, false)
            }
        } else {
            (bytes, false)
        };
        let stored = blob.len() as u64;
        let computable = is_computable(&relative);
        cursor = align_up(cursor, BLOB_ALIGN);
        entries.push(Entry {
            path: relative,
            bucket,
            computable,
            size,
            sha256,
            offset: cursor,
            stored,
            compressed,
        });
        cursor += stored;
        blobs.push(blob);
    }

    let content: u64 = entries.iter().map(|entry| entry.size).sum();
    let manifest = Manifest {
        format: FORMAT,
        minecraft: options.minecraft.clone(),
        output_version: options.output_version,
        schema: options.schema,
        created_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        total_bytes: content,
        blob_bytes: cursor,
        omitted,
        entries,
    };
    let encoded = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
    let blob_base = align_up(HEADER_LEN + encoded.len() as u64, ALIGN);

    let mut file = File::create(out).map_err(|e| format!("{}: {e}", out.display()))?;
    file.write_all(&MAGIC).map_err(|e| e.to_string())?;
    file.write_all(&(encoded.len() as u64).to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&encoded).map_err(|e| e.to_string())?;
    let padding = (blob_base - HEADER_LEN - encoded.len() as u64) as usize;
    file.write_all(&vec![0u8; padding]).map_err(|e| e.to_string())?;

    for (entry, blob) in manifest.entries.iter().zip(blobs.iter()) {
        file.seek(SeekFrom::Start(blob_base + entry.offset))
            .map_err(|e| e.to_string())?;
        file.write_all(blob).map_err(|e| e.to_string())?;
    }
    file.flush().map_err(|e| e.to_string())?;
    Ok(manifest)
}
/// A packed archive, open for reading one blob at a time.
pub struct Reader {
    file: File,
    manifest: Manifest,
    blob_base: u64,
}

impl Reader {
    pub fn open(path: &Path) -> Result<Reader, String> {
        let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut header = [0u8; HEADER_LEN as usize];
        file.read_exact(&mut header)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if header[..8] != MAGIC {
            return Err(format!("{}: not an HZR3 v1 archive", path.display()));
        }
        // HEADER_LEN is 16, so this slice is always eight bytes; taking it as a
// fallible conversion keeps the device free of a panic path all the same.
            let length = header[8..16]
                .try_into()
                .map(u64::from_le_bytes)
                .map_err(|_| format!("{}: header is too short", path.display()))?;
        if length == 0 || length > MAX_MANIFEST {
            return Err(format!(
                "{}: manifest length {length} is out of range",
                path.display()
            ));
        }
        let mut encoded = vec![0u8; length as usize];
        file.read_exact(&mut encoded)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let manifest: Manifest =
            serde_json::from_slice(&encoded).map_err(|e| format!("{}: {e}", path.display()))?;
        if manifest.format != FORMAT {
            return Err(format!(
                "{}: archive format {}, this crate reads {FORMAT}",
                path.display(),
                manifest.format
            ));
        }
        let blob_base = align_up(HEADER_LEN + length, ALIGN);
        Ok(Reader {
            file,
            manifest,
            blob_base,
        })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Where the blob section starts in the file.
    pub fn blob_base(&self) -> u64 {
        self.blob_base
    }

    /// The blob's bytes, copied out.
    pub fn read(&mut self, index: usize) -> Result<Vec<u8>, String> {
        let entry = self
            .manifest
            .entries
            .get(index)
            .ok_or_else(|| format!("no entry {index}"))?
            .clone();
        let stored = if entry.stored == 0 && !entry.compressed {
            entry.size
        } else {
            entry.stored
        };
        let mut data = vec![0u8; stored as usize];
        self.file
            .seek(SeekFrom::Start(self.blob_base + entry.offset))
            .map_err(|e| e.to_string())?;
        self.file.read_exact(&mut data).map_err(|e| e.to_string())?;
        if entry.compressed {
            return inflate(&data, entry.size);
        }
        Ok(data)
    }

    /// Hash every blob and check the file really holds what the manifest says.
    /// Catches truncation, a corrupted copy, and a manifest from another archive.
    pub fn verify(&mut self) -> Result<Report, String> {
        let mut report = Report::default();
        let file_len = self.file.metadata().map_err(|e| e.to_string())?.len();
        for (index, entry) in self.manifest.entries.iter().enumerate() {
            if entry.sha256.len() != 64 {
                return Err(format!("entry {index} ({}): malformed sha256", entry.path));
            }
            let start = self.blob_base + entry.offset;
            let stored = if entry.stored == 0 && !entry.compressed {
                entry.size
            } else {
                entry.stored
            };
            if start + stored > file_len {
                return Err(format!(
                    "entry {index} ({}): runs past the end of the archive",
                    entry.path
                ));
            }
            self.file
                .seek(SeekFrom::Start(start))
                .map_err(|e| e.to_string())?;
            let mut data = vec![0u8; stored as usize];
            self.file.read_exact(&mut data).map_err(|e| e.to_string())?;
            let content = if entry.compressed {
                inflate(&data, entry.size)?
            } else {
                data
            };
            if content.len() as u64 != entry.size {
                return Err(format!(
                    "entry {index} ({}): {} bytes, manifest says {}",
                    entry.path,
                    content.len(),
                    entry.size
                ));
            }
            let mut hasher = Sha256::new();
            hasher.update(&content);
            let found = hex(&hasher.finalize());
            if found != entry.sha256 {
                return Err(format!(
                    "entry {index} ({}): sha256 {found}, manifest says {}",
                    entry.path, entry.sha256
                ));
            }
            report.files += 1;
            report.bytes += entry.size;
            let travelled = if entry.stored == 0 && !entry.compressed {
                entry.size
            } else {
                entry.stored
            };
            if entry.computable {
                report.computable_bytes += travelled;
            } else {
                report.packed_bytes += travelled;
            }
            *report
                .per_bucket
                .entry(entry.bucket.letter())
                .or_insert(0) += 1;
        }
        report.missing_minecraft = self.manifest.minecraft == "unknown";
        Ok(report)
    }
}

/// Write every blob under `out`, creating the directories the paths imply.
pub fn extract(archive: &Path, out: &Path) -> Result<usize, String> {
    let mut reader = Reader::open(archive)?;
    let count = reader.manifest().entries.len();
    for index in 0..count {
        let entry = reader.manifest().entries[index].clone();
        if entry
            .path
            .split('/')
            .any(|part| part == ".." || part.is_empty())
        {
            return Err(format!(
                "entry {index}: refusing unsafe path {:?}",
                entry.path
            ));
        }
        let target = out.join(entry.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let data = reader.read(index)?;
        let mut file = File::create(&target).map_err(|e| format!("{}: {e}", target.display()))?;
        file.write_all(&data)
            .map_err(|e| format!("{}: {e}", target.display()))?;
    }
    Ok(count)
}

/// `pack`, returning the archive path for callers that chain.
pub fn pack_to(dir: &Path, out: &Path, options: &PackOptions) -> Result<PathBuf, String> {
    pack(dir, out, options)?;
    Ok(out.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Options without compression, for the byte-level corruption tests.
    fn plain() -> PackOptions {
        PackOptions {
            compress: false,
            ..PackOptions::default()
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hzr-pack-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, relative: &str, data: &[u8]) {
        let path = dir.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, data).unwrap();
    }

    #[test]
    fn a_packed_directory_verifies_and_extracts_byte_for_byte() {
        let root = scratch("roundtrip");
        let source = root.join("in");
        write(&source, "facts.json", b"{\"states\":[]}");
        write(&source, "assets/minecraft/pack.mcmeta", b"{}");
        write(&source, "worldgen/biome/plains.json", b"{}");
        write(&source, "export.log", b"noise");
        let archive = root.join("out.hzr3");

        let options = PackOptions {
            minecraft: "26.3".into(),
            output_version: 4,
            schema: 7,
            include_droppable: false,
            compress: true,
            only_packed: false,
            omit: Vec::new(),
        };
        let manifest = pack(&source, &archive, &options).unwrap();
        // The log is droppable and was left out.
        assert_eq!(manifest.entries.len(), 3);
        // facts.json must travel; the assets and the worldgen JSON are derivable.
        let facts = manifest
            .entries
            .iter()
            .find(|e| e.path == "facts.json")
            .unwrap();
        assert!(!facts.computable);
        assert_eq!(facts.bucket, Bucket::BootCritical);
        let worldgen = manifest
            .entries
            .iter()
            .find(|e| e.path == "worldgen/biome/plains.json")
            .unwrap();
        assert!(worldgen.computable);
        assert_eq!(worldgen.bucket, Bucket::BootCritical);

        let mut reader = Reader::open(&archive).unwrap();
        assert_eq!(reader.manifest().minecraft, "26.3");
        let report = reader.verify().unwrap();
        assert_eq!(report.files, 3);
        assert_eq!(report.per_bucket.get("A"), Some(&3));
        assert_eq!(report.computable_bytes, 4);
        assert_eq!(report.packed_bytes, 13);

        let restored = root.join("out");
        assert_eq!(extract(&archive, &restored).unwrap(), 3);
        assert_eq!(
            fs::read(restored.join("facts.json")).unwrap(),
            b"{\"states\":[]}"
        );
        assert_eq!(
            fs::read(restored.join("worldgen/biome/plains.json")).unwrap(),
            b"{}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn compression_round_trips_and_a_bad_blob_is_caught() {
        let root = scratch("deflate");
        let source = root.join("in");
        // Highly repetitive text, which is what the prepared tables look like.
        let body = "minecraft:stone ".repeat(2000);
        write(&source, "big.json", body.as_bytes());
        let archive = root.join("out.hzr3");
        let options = PackOptions {
            compress: true,
            ..PackOptions::default()
        };
        let manifest = pack(&source, &archive, &options).unwrap();
        assert!(manifest.entries[0].compressed);
        assert!(manifest.entries[0].stored < manifest.entries[0].size);

        let mut reader = Reader::open(&archive).unwrap();
        assert!(reader.verify().is_ok());
        let restored = reader.read(0).unwrap();
        assert_eq!(restored, body.as_bytes());

        // A flipped byte in the deflate stream must be caught, not returned.
        let mut bytes = fs::read(&archive).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        fs::write(&archive, &bytes).unwrap();
        let mut broken = Reader::open(&archive).unwrap();
        assert!(broken.verify().is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_flipped_byte_fails_verification() {
        let root = scratch("corrupt");
        let source = root.join("in");
        write(&source, "facts.json", &vec![7u8; 4096]);
        let archive = root.join("out.hzr3");
        pack(&source, &archive, &plain()).unwrap();

        let mut bytes = fs::read(&archive).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        fs::write(&archive, &bytes).unwrap();

        let mut reader = Reader::open(&archive).unwrap();
        let error = reader.verify().unwrap_err();
        assert!(error.contains("sha256"), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_truncated_archive_is_refused_rather_than_read_short() {
        let root = scratch("truncate");
        let source = root.join("in");
        write(&source, "facts.json", &vec![1u8; 8192]);
        let archive = root.join("out.hzr3");
        pack(&source, &archive, &plain()).unwrap();

        let bytes = fs::read(&archive).unwrap();
        fs::write(&archive, &bytes[..bytes.len() - 100]).unwrap();

        let mut reader = Reader::open(&archive).unwrap();
        let error = reader.verify().unwrap_err();
        assert!(error.contains("past the end"), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn blobs_start_on_eight_byte_boundaries_in_order() {
        let root = scratch("align");
        let source = root.join("in");
        // Sizes that are not multiples of eight force padding between blobs.
        write(&source, "a.json", b"123");
        write(&source, "b.json", b"12345");
        write(&source, "c.json", b"1234567");
        let archive = root.join("out.hzr3");
        let manifest = pack(&source, &archive, &PackOptions::default()).unwrap();

        let mut cursor = 0;
        for entry in &manifest.entries {
            let aligned = align_up(cursor, BLOB_ALIGN);
            assert_eq!(entry.offset % BLOB_ALIGN, 0, "{} is not aligned", entry.path);
            assert_eq!(
                entry.offset, aligned,
                "{} is not where it should be",
                entry.path
            );
            cursor = entry.offset + entry.size;
        }
        // Content bytes exclude the padding the blob section carries.
        assert_eq!(manifest.total_bytes, 3 + 5 + 7);
        assert_eq!(manifest.blob_bytes, cursor);
        assert!(manifest.blob_bytes > manifest.total_bytes);
        // The file is exactly as long as the manifest says, no trailing slack.
        let reader = Reader::open(&archive).unwrap();
        assert_eq!(
            reader.file.metadata().unwrap().len(),
            reader.blob_base() + manifest.blob_bytes
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn classification_matches_the_audit() {
        assert_eq!(classify("gameplay-facts.json"), Bucket::BootCritical);
        assert_eq!(classify("occlusion.nbt"), Bucket::BootCritical);
        assert_eq!(
            classify("biome-parameters/overworld.json"),
            Bucket::BootCritical
        );
        assert_eq!(classify("entity-models.json"), Bucket::Degradable);
        assert_eq!(classify("damage-tags.json"), Bucket::Degradable);
        assert_eq!(classify("export.log"), Bucket::Droppable);
        assert_eq!(classify("horizon-sprites.cache"), Bucket::Droppable);
        assert_eq!(classify("mystery.bin"), Bucket::Unknown);

        // Required but derivable on device: the two axes disagree on purpose.
        for path in [
            "assets/minecraft/pack.mcmeta",
            "data/minecraft/tags/block/mineable/pickaxe.json",
            "worldgen/biome/plains.json",
            "asset-index.json",
            "pack.png",
        ] {
            assert_eq!(classify(path), Bucket::BootCritical, "{path}");
            assert!(is_computable(path), "{path}");
        }
        assert!(is_computable("data/minecraft/recipe/stick.json"));
        // prepare writes this one with its own formatting, so it ships as is.
        assert_eq!(classify("block-tags.json"), Bucket::BootCritical);
        assert!(!is_computable("block-tags.json"));
        // The launcher's own manifests are read at boot, so they travel.
        for manifest in ["horizon-resources.json", "horizon-prepare.json"] {
            assert_eq!(classify(manifest), Bucket::Unknown, "{manifest}");
        }
        assert!(!is_computable("facts.json"));
        assert!(!is_computable("entity-models.json"));

        // `worldgen/` splits: the jar's data tree is derivable, the data
        // generator's tables and the block dumps are not (29 files in a real
        // prepare, measured in route3/AUDIT.md).
        assert!(is_computable("worldgen/biome/plains.json"));
        assert!(is_computable("worldgen/structure/minecraft/village/plains/houses/plains_small_house_1.nbt"));
        assert!(!is_computable("worldgen/biomes.json"));
        assert!(!is_computable("worldgen/noises.json"));
        assert!(!is_computable("worldgen/blocks/cached-collision.nbt"));
        assert!(!is_computable("worldgen/biome_parameters/overworld.json"));
    }

    #[test]
    fn an_omitted_file_is_left_out_and_recorded() {
        let root = scratch("omit");
        let source = root.join("in");
        write(&source, "facts.json", b"{}");
        write(&source, "model-states.json", b"{\"a\":{}}");
        let archive = root.join("out.hzr3");
        let options = PackOptions {
            omit: vec!["model-states.json".to_string()],
            ..PackOptions::default()
        };
        let manifest = pack(&source, &archive, &options).unwrap();
        assert_eq!(manifest.entries.len(), 1);
        assert_eq!(manifest.omitted, vec!["model-states.json".to_string()]);

        let reader = Reader::open(&archive).unwrap();
        assert_eq!(reader.manifest().omitted, vec!["model-states.json".to_string()]);
        assert!(!reader
            .manifest()
            .entries
            .iter()
            .any(|entry| entry.path == "model-states.json"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn two_runs_over_one_directory_agree() {
        let root = scratch("determinism");
        let source = root.join("in");
        write(&source, "facts.json", b"{}");
        write(&source, "items.json", b"{}");
        let first = root.join("a.hzr3");
        let second = root.join("b.hzr3");
        pack(&source, &first, &PackOptions::default()).unwrap();
        pack(&source, &second, &PackOptions::default()).unwrap();
        let a = Reader::open(&first).unwrap().manifest().clone();
        let b = Reader::open(&second).unwrap().manifest().clone();
        let paths = |m: &Manifest| -> Vec<(String, String)> {
            m.entries
                .iter()
                .map(|e| (e.path.clone(), e.sha256.clone()))
                .collect()
        };
        assert_eq!(paths(&a), paths(&b));
        assert_eq!(a.total_bytes, b.total_bytes);
        let _ = fs::remove_dir_all(root);
    }
}
