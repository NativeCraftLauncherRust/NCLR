//! A small NBT reader and writer.
//!
//! The block tables are NBT compounds; comparing ours against theirs means
//! reading both. The writer exists so the reader can be proved: parse a table
//! and write it back, and the bytes must be identical.
//!
//! Two details matter for that round trip:
//!
//! - a compound keeps its keys **in order** (`Vec`, not a map);
//! - a list keeps its element type, which an empty list still has to name.

use std::fmt;

/// An NBT tag.
#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    List { element: u8, items: Vec<Tag> },
    Compound(Vec<(String, Tag)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Tag {
    /// The tag's id, as written before its payload.
    pub fn id(&self) -> u8 {
        match self {
            Tag::Byte(_) => 1,
            Tag::Short(_) => 2,
            Tag::Int(_) => 3,
            Tag::Long(_) => 4,
            Tag::Float(_) => 5,
            Tag::Double(_) => 6,
            Tag::ByteArray(_) => 7,
            Tag::String(_) => 8,
            Tag::List { .. } => 9,
            Tag::Compound(_) => 10,
            Tag::IntArray(_) => 11,
            Tag::LongArray(_) => 12,
        }
    }

    pub fn as_compound(&self) -> Option<&Vec<(String, Tag)>> {
        match self {
            Tag::Compound(entries) => Some(entries),
            _ => None,
        }
    }

    pub fn get(&self, name: &str) -> Option<&Tag> {
        self.as_compound()?
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }
}

#[derive(Debug)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn need(&self, count: usize) -> Result<(), ParseError> {
        if self.at + count > self.bytes.len() {
            return Err(ParseError(format!(
                "truncated at {}: needs {count} bytes",
                self.at
            )));
        }
        Ok(())
    }

    fn u8(&mut self) -> Result<u8, ParseError> {
        self.need(1)?;
        let value = self.bytes[self.at];
        self.at += 1;
        Ok(value)
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], ParseError> {
        self.need(count)?;
        let slice = &self.bytes[self.at..self.at + count];
        self.at += count;
        Ok(slice)
    }

    fn i16(&mut self) -> Result<i16, ParseError> {
        Ok(i16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn i32(&mut self) -> Result<i32, ParseError> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn i64(&mut self) -> Result<i64, ParseError> {
        Ok(i64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }

    /// A length-prefixed string. Negative lengths are refused.
    fn string(&mut self) -> Result<String, ParseError> {
        let length = self.i16()?;
        if length < 0 {
            return Err(ParseError(format!("negative string length {length}")));
        }
        let bytes = self.take(length as usize)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|error| ParseError(format!("string is not UTF-8: {error}")))
    }

    fn payload(&mut self, id: u8) -> Result<Tag, ParseError> {
        Ok(match id {
            1 => Tag::Byte(self.u8()? as i8),
            2 => Tag::Short(self.i16()?),
            3 => Tag::Int(self.i32()?),
            4 => Tag::Long(self.i64()?),
            5 => Tag::Float(f32::from_bits(self.i32()? as u32)),
            6 => Tag::Double(f64::from_bits(self.i64()? as u64)),
            7 => {
                let length = self.i32()?;
                if length < 0 {
                    return Err(ParseError(format!("negative byte array {length}")));
                }
                let bytes = self.take(length as usize)?;
                Tag::ByteArray(bytes.iter().map(|b| *b as i8).collect())
            }
            8 => Tag::String(self.string()?),
            9 => {
                let element = self.u8()?;
                let length = self.i32()?;
                if length < 0 {
                    return Err(ParseError(format!("negative list {length}")));
                }
                let mut items = Vec::with_capacity(length.min(4096) as usize);
                for _ in 0..length {
                    items.push(self.payload(element)?);
                }
                Tag::List { element, items }
            }
            10 => {
                let mut entries = Vec::new();
                loop {
                    let kind = self.u8()?;
                    if kind == 0 {
                        break;
                    }
                    let name = self.string()?;
                    let value = self.payload(kind)?;
                    entries.push((name, value));
                }
                Tag::Compound(entries)
            }
            11 => {
                let length = self.i32()?;
                if length < 0 {
                    return Err(ParseError(format!("negative int array {length}")));
                }
                let mut values = Vec::with_capacity(length as usize);
                for _ in 0..length {
                    values.push(self.i32()?);
                }
                Tag::IntArray(values)
            }
            12 => {
                let length = self.i32()?;
                if length < 0 {
                    return Err(ParseError(format!("negative long array {length}")));
                }
                let mut values = Vec::with_capacity(length as usize);
                for _ in 0..length {
                    values.push(self.i64()?);
                }
                Tag::LongArray(values)
            }
            other => return Err(ParseError(format!("unknown tag id {other}"))),
        })
    }
}

/// Parse a whole file: a named root compound.
pub fn parse(bytes: &[u8]) -> Result<(String, Tag), ParseError> {
    let mut reader = Reader { bytes, at: 0 };
    let id = reader.u8()?;
    if id != 10 {
        return Err(ParseError(format!("root is tag {id}, not a compound")));
    }
    let name = reader.string()?;
    let value = reader.payload(10)?;
    Ok((name, value))
}

fn put_i16(out: &mut Vec<u8>, value: i16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn put_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn put_i64(out: &mut Vec<u8>, value: i64) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn put_string(out: &mut Vec<u8>, text: &str) {
    put_i16(out, text.len() as i16);
    out.extend_from_slice(text.as_bytes());
}

fn put_payload(out: &mut Vec<u8>, tag: &Tag) {
    match tag {
        Tag::Byte(value) => out.push(*value as u8),
        Tag::Short(value) => put_i16(out, *value),
        Tag::Int(value) => put_i32(out, *value),
        Tag::Long(value) => put_i64(out, *value),
        Tag::Float(value) => put_i32(out, value.to_bits() as i32),
        Tag::Double(value) => put_i64(out, value.to_bits() as i64),
        Tag::ByteArray(values) => {
            put_i32(out, values.len() as i32);
            out.extend(values.iter().map(|b| *b as u8));
        }
        Tag::String(text) => put_string(out, text),
        Tag::List { element, items } => {
            out.push(*element);
            put_i32(out, items.len() as i32);
            for item in items {
                put_payload(out, item);
            }
        }
        Tag::Compound(entries) => {
            for (name, value) in entries {
                out.push(value.id());
                put_string(out, name);
                put_payload(out, value);
            }
            out.push(0);
        }
        Tag::IntArray(values) => {
            put_i32(out, values.len() as i32);
            for value in values {
                put_i32(out, *value);
            }
        }
        Tag::LongArray(values) => {
            put_i32(out, values.len() as i32);
            for value in values {
                put_i64(out, *value);
            }
        }
    }
}

/// Write a file back, exactly as `parse` read it when round-tripped.
pub fn write(name: &str, root: &Tag) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(10);
    put_string(&mut out, name);
    put_payload(&mut out, root);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_compound_round_trips() {
        let root = Tag::Compound(vec![
            ("DataVersion".to_string(), Tag::Int(4189961)),
            (
                "names".to_string(),
                Tag::List {
                    element: 8,
                    items: vec![Tag::String("minecraft:air".into())],
                },
            ),
            (
                "bits".to_string(),
                Tag::LongArray(vec![-1, 0, 0, 7]),
            ),
            ("empty".to_string(), Tag::List { element: 0, items: vec![] }),
        ]);
        let bytes = write("root", &root);
        let (name, parsed) = parse(&bytes).unwrap();
        assert_eq!(name, "root");
        assert_eq!(parsed, root);
        assert_eq!(write(&name, &parsed), bytes);
    }

    #[test]
    fn a_truncated_file_is_an_error() {
        let bytes = write("root", &Tag::Compound(vec![("a".into(), Tag::Int(1))]));
        assert!(parse(&bytes[..bytes.len() - 1]).is_err());
    }
}
