//! The `states` trailer of a block table, split out.
//!
//! `cached-collision.nbt`, `occlusion.nbt`, `cached-support.nbt`,
//! `block-properties.nbt` and `movement-properties.nbt` each end with a `states`
//! list: one entry per block state, naming it, so the reader can check the file
//! against the registry. That list is **4,173,554 bytes**, byte-identical in all
//! five (measured: `route3/AUDIT.md`), and it is always the last key of the root
//! compound.
//!
//! NBT compounds carry no length prefix — only a terminator — so that trailer can
//! be cut off and put back without touching a single other byte:
//!
//! ```text
//! file      = [cached_shapes .. DataVersion][states entry][0x00]
//! stripped  = [cached_shapes .. DataVersion][0x00]
//! blob      = [states entry]
//! ```

use std::path::Path;

/// One key of the root compound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// The tag byte's offset.
    pub start: usize,
    /// One past the payload's last byte.
    pub end: usize,
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn need(&self, count: usize) -> Result<(), String> {
        if self.at + count > self.bytes.len() {
            return Err(format!(
                "truncated at {}: needs {count} bytes",
                self.at
            ));
        }
        Ok(())
    }
    fn u8(&mut self) -> Result<u8, String> {
        self.need(1)?;
        let value = self.bytes[self.at];
        self.at += 1;
        Ok(value)
    }
    fn u16(&mut self) -> Result<u16, String> {
        self.need(2)?;
        let value = u16::from_be_bytes([self.bytes[self.at], self.bytes[self.at + 1]]);
        self.at += 2;
        Ok(value)
    }
    fn i32(&mut self) -> Result<i32, String> {
        self.need(4)?;
        let mut word = [0u8; 4];
        word.copy_from_slice(&self.bytes[self.at..self.at + 4]);
        self.at += 4;
        Ok(i32::from_be_bytes(word))
    }
    fn skip(&mut self, count: usize) -> Result<(), String> {
        self.need(count)?;
        self.at += count;
        Ok(())
    }
    fn string(&mut self) -> Result<String, String> {
        let length = self.u16()? as usize;
        self.need(length)?;
        let text = std::str::from_utf8(&self.bytes[self.at..self.at + length])
            .map_err(|e| format!("a name is not utf-8 at {}: {e}", self.at))?
            .to_string();
        self.at += length;
        Ok(text)
    }
    /// Read one payload of tag type `tag`.
    fn payload(&mut self, tag: u8) -> Result<(), String> {
        match tag {
            1 => self.skip(1),
            2 => self.skip(2),
            3 => self.skip(4),
            4 => self.skip(8),
            5 => self.skip(4),
            6 => self.skip(8),
            7 => {
                let length = self.i32()?;
                if length < 0 {
                    return Err(format!("negative byte-array length at {}", self.at));
                }
                self.skip(length as usize)
            }
            8 => self.string().map(|_| ()),
            9 => {
                // A list's element type comes first; an empty list may name any.
                let element = self.u8()?;
                let length = self.i32()?;
                if length < 0 {
                    return Err(format!("negative list length at {}", self.at));
                }
                if element == 0 && length > 0 {
                    return Err(format!("list of TAG_End at {}", self.at));
                }
                for _ in 0..length {
                    self.payload(element)?;
                }
                Ok(())
            }
            10 => {
                loop {
                    let inner = self.u8()?;
                    if inner == 0 {
                        return Ok(());
                    }
                    self.string()?;
                    self.payload(inner)?;
                }
            }
            11 => {
                let length = self.i32()?;
                if length < 0 || length as usize > self.bytes.len() {
                    return Err(format!("bad int-array length at {}", self.at));
                }
                self.skip(length as usize * 4)
            }
            12 => {
                let length = self.i32()?;
                if length < 0 || length as usize > self.bytes.len() {
                    return Err(format!("bad long-array length at {}", self.at));
                }
                self.skip(length as usize * 8)
            }
            other => Err(format!("unknown tag {other} at {}", self.at)),
        }
    }
}

/// The keys of the root compound, with their byte ranges.
pub fn entries(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    let mut reader = Reader { bytes, at: 0 };
    if reader.u8()? != 10 {
        return Err("the root is not a compound".into());
    }
    reader.string()?;
    let mut out = Vec::new();
    loop {
        let start = reader.at;
        let tag = reader.u8()?;
        if tag == 0 {
            return Ok(out);
        }
        let name = reader.string()?;
        reader.payload(tag)?;
        out.push(Entry {
            name,
            start,
            end: reader.at,
        });
    }
}

/// `(stripped, blob)` from a table whose last key is `states`.
pub fn strip(bytes: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
    let list = entries(bytes)?;
    let last = list.last().ok_or("the root compound is empty")?;
    if last.name != "states" {
        return Err(format!(
            "the last key is {}, not states; this table has no trailer to split",
            last.name
        ));
    }
    if last.end != bytes.len() - 1 || bytes[bytes.len() - 1] != 0 {
        return Err("the states entry does not end at the compound's terminator".into());
    }
    let mut stripped = Vec::with_capacity(last.start + 1);
    stripped.extend_from_slice(&bytes[..last.start]);
    stripped.push(0);
    Ok((stripped, bytes[last.start..last.end].to_vec()))
}

/// Put `blob` back before the compound's terminator.
pub fn restore(stripped: &[u8], blob: &[u8]) -> Result<Vec<u8>, String> {
    if stripped.last() != Some(&0) {
        return Err("the stripped table does not end with a terminator".into());
    }
    let mut out = Vec::with_capacity(stripped.len() + blob.len());
    out.extend_from_slice(&stripped[..stripped.len() - 1]);
    out.extend_from_slice(blob);
    out.push(0);
    Ok(out)
}

/// Strip `source` into `stripped`, writing the trailer to `blob`. Returns both sizes.
pub fn strip_file(source: &Path, stripped: &Path, blob: &Path) -> Result<(usize, usize), String> {
    let bytes = std::fs::read(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let (body, trailer) = strip(&bytes)?;
    std::fs::write(stripped, &body).map_err(|e| format!("{}: {e}", stripped.display()))?;
    std::fs::write(blob, &trailer).map_err(|e| format!("{}: {e}", blob.display()))?;
    Ok((body.len(), trailer.len()))
}

/// Put `blob` back into `stripped`, writing `out`.
pub fn restore_file(stripped: &Path, blob: &Path, out: &Path) -> Result<usize, String> {
    let body = std::fs::read(stripped).map_err(|e| format!("{}: {e}", stripped.display()))?;
    let trailer = std::fs::read(blob).map_err(|e| format!("{}: {e}", blob.display()))?;
    let whole = restore(&body, &trailer)?;
    std::fs::write(out, &whole).map_err(|e| format!("{}: {e}", out.display()))?;
    Ok(whole.len())
}

/// A top-level key cut out of a compound, with the position it came from.
#[derive(Debug, Clone)]
pub struct KeySpan {
    /// Its index among the root compound's keys.
    pub index: usize,
    /// Its exact bytes: the tag, the name and the payload.
    pub bytes: Vec<u8>,
}

/// Remove the top-level key `name`, keeping its bytes and position.
///
/// NBT compounds are a sequence of keys with no count and no separators, so
/// deleting one is a splice; putting it back at the same index restores the
/// original byte for byte.
pub fn strip_key(bytes: &[u8], name: &str) -> Result<(Vec<u8>, KeySpan), String> {
    let list = entries(bytes)?;
    let (index, entry) = list
        .iter()
        .enumerate()
        .find(|(_, entry)| entry.name == name)
        .ok_or_else(|| format!("no top-level key {name}"))?;
    let mut body = Vec::with_capacity(bytes.len() - (entry.end - entry.start));
    body.extend_from_slice(&bytes[..entry.start]);
    body.extend_from_slice(&bytes[entry.end..]);
    Ok((
        body,
        KeySpan {
            index,
            bytes: bytes[entry.start..entry.end].to_vec(),
        },
    ))
}

/// Put a key back at the index it came from.
pub fn restore_key(body: &[u8], span: &KeySpan) -> Result<Vec<u8>, String> {
    if body.last() != Some(&0) {
        return Err("the compound does not end with a terminator".into());
    }
    let list = entries(body)?;
    let at = if span.index >= list.len() {
        // It was the last key: it goes back before the terminator.
        body.len() - 1
    } else {
        list[span.index].start
    };
    let mut out = Vec::with_capacity(body.len() + span.bytes.len());
    out.extend_from_slice(&body[..at]);
    out.extend_from_slice(&span.bytes);
    out.extend_from_slice(&body[at..]);
    Ok(out)
}

/// `[u32 index][key bytes]`, the on-disk form of a stripped key.
pub fn encode_span(span: &KeySpan) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + span.bytes.len());
    out.extend_from_slice(&(span.index as u32).to_le_bytes());
    out.extend_from_slice(&span.bytes);
    out
}

/// Read `[u32 index][key bytes]` back.
pub fn decode_span(bytes: &[u8]) -> Result<KeySpan, String> {
    if bytes.len() < 5 {
        return Err("a key file needs at least a tag and an index".into());
    }
    let mut word = [0u8; 4];
    word.copy_from_slice(&bytes[..4]);
    Ok(KeySpan {
        index: u32::from_le_bytes(word) as usize,
        bytes: bytes[4..].to_vec(),
    })
}

/// `block-properties.nbt`'s `occlusion` key is `occlusion.nbt`'s whole body.
///
/// Measured: the key's payload is 6,180,751 bytes, byte-identical to
/// `occlusion.nbt` from offset 3 (after its three-byte root header) to the end,
/// terminator included. So the copy ships zero bytes: it is rebuilt from the
/// table that goes along anyway.
pub const OCCLUSION_KEY: &str = "occlusion";

/// The `occlusion` key's bytes, built from `occlusion.nbt`'s file.
pub fn occlusion_key(occlusion_file: &[u8]) -> Result<Vec<u8>, String> {
    if occlusion_file.len() < 4 || occlusion_file[0] != 10 {
        return Err("occlusion.nbt is not a compound".into());
    }
    let mut key = Vec::with_capacity(12 + occlusion_file.len() - 3);
    key.push(10); // compound
    key.extend_from_slice(&(OCCLUSION_KEY.len() as u16).to_be_bytes());
    key.extend_from_slice(OCCLUSION_KEY.as_bytes());
    key.extend_from_slice(&occlusion_file[3..]);
    Ok(key)
}

/// The files whose `states` trailer is the same one, in the order measured.
pub const SHARED_TRAILER_TABLES: [&str; 5] = [
    "occlusion.nbt",
    "cached-collision.nbt",
    "cached-support.nbt",
    "block-properties.nbt",
    "movement-properties.nbt",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// A compound with an int, a list of ints, and optionally the states trailer.
    fn table(with_states: bool) -> Vec<u8> {
        let mut bytes = vec![10, 0, 0]; // compound, empty name
        // "a": int
        bytes.extend_from_slice(&[3, 0, 1, b'a', 0, 0, 0, 7]);
        // "shapes": list of int, one element
        bytes.extend_from_slice(&[9, 0, 6]);
        bytes.extend_from_slice(b"shapes");
        bytes.extend_from_slice(&[3, 0, 0, 0, 1, 0, 0, 0, 9]);
        if with_states {
            // "states": list of compound, one element
            bytes.extend_from_slice(&[9, 0, 6]);
            bytes.extend_from_slice(b"states");
            bytes.extend_from_slice(&[10, 0, 0, 0, 1]);
            bytes.extend_from_slice(&[8, 0, 4]);
            bytes.extend_from_slice(b"Name");
            bytes.extend_from_slice(&[0, 4]);
            bytes.extend_from_slice(b"mine");
            bytes.push(0); // end of the inner compound
        }
        bytes.push(0); // end of the root compound
        bytes
    }

    #[test]
    fn a_middle_key_can_be_lifted_and_put_back() {
        let bytes = table(true);
        let (body, span) = strip_key(&bytes, "shapes").unwrap();
        assert_eq!(span.index, 1);
        assert_eq!(body.len() + span.bytes.len(), bytes.len());
        assert_eq!(restore_key(&body, &span).unwrap(), bytes);
        // The on-disk form carries the index.
        let encoded = encode_span(&span);
        let decoded = decode_span(&encoded).unwrap();
        assert_eq!(decoded.index, 1);
        assert_eq!(decoded.bytes, span.bytes);
    }

    #[test]
    fn the_last_key_goes_back_before_the_terminator() {
        let bytes = table(true);
        let (body, span) = strip_key(&bytes, "states").unwrap();
        assert_eq!(span.index, 2);
        assert_eq!(body.last(), Some(&0));
        assert_eq!(restore_key(&body, &span).unwrap(), bytes);
    }

    #[test]
    fn an_absent_key_names_what_was_asked_for() {
        let error = strip_key(&table(true), "nope").unwrap_err();
        assert!(error.contains("nope"), "{error}");
    }

    #[test]
    fn a_table_reports_its_keys_with_their_ranges() {
        let bytes = table(true);
        let list = entries(&bytes).unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].name, "a");
        assert_eq!(list[2].name, "states");
        assert_eq!(list[2].end, bytes.len() - 1);
    }

    #[test]
    fn stripping_and_restoring_gives_the_original_bytes_back() {
        let bytes = table(true);
        let (body, blob) = strip(&bytes).unwrap();
        // The body keeps the compound's terminator, which the blob does not carry.
        assert_eq!(body.len() + blob.len(), bytes.len());
        assert_eq!(body.last(), Some(&0));
        assert_eq!(restore(&body, &blob).unwrap(), bytes);
    }

    #[test]
    fn a_table_whose_last_key_is_not_states_is_refused() {
        let bytes = table(false);
        let error = strip(&bytes).unwrap_err();
        assert!(error.contains("not states"), "{error}");
        assert!(restore(&[1, 2], &[3]).is_err(), "a body must end with a terminator");
    }

    #[test]
    fn a_truncated_nbt_is_an_error_not_a_panic() {
        let bytes = table(true);
        for cut in [1, 4, 9, bytes.len() - 2] {
            assert!(entries(&bytes[..cut]).is_err(), "cut at {cut}");
        }
    }
}
