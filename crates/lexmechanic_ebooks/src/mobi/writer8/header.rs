//! Ordered-field binary header builder.
//!
//! Port of `calibre.ebooks.mobi.writer8.header.Header`. Python's version is
//! a generic `OrderedDict` subclass whose field list is parsed out of a
//! multi-line string DSL (`DEFINITION`) with `eval()`. Rust has neither
//! `OrderedDict`-by-string-parsing nor `eval`, so this is ported as a
//! small builder over an explicit, ordered `Vec` of `(name, value)` pairs
//! instead: callers construct a [`Header`] from a `&[FieldDef]` (the
//! Rust-side equivalent of a parsed `DEFINITION`), `set()` the dynamic
//! fields, then `build()` the flat buffer. [`crate::mobi::writer8::index::IndexHeader`]
//! and [`crate::mobi::writer8::mobi::MOBIHeader`] both build their field
//! lists this way, mirroring `IndexHeader`/`MOBIHeader` subclassing
//! `Header` in Python.

use std::collections::HashMap;

use anyhow::{bail, Result};

use crate::mobi::utils::align_block;

/// A field's value, mirroring the possible values in a Python
/// `DEFINITION` line: `DYN` (must be supplied before `build()`), a byte
/// string (`zeroes(n)`, `nulls(n)`, or a literal like `b'MOBI'`), or an
/// integer packed big-endian at build time.
#[derive(Debug, Clone)]
pub enum FieldValue {
    /// Not yet set (Python's `DYN` / `None`). `build()` errors if any
    /// field is still `Dyn`.
    Dyn,
    Bytes(Vec<u8>),
    Int(u64),
}

/// `zeroes(x)` in `header.py`: `x` NUL bytes.
pub fn zeroes(n: usize) -> FieldValue {
    FieldValue::Bytes(vec![0u8; n])
}

/// `nulls(x)` in `header.py`: `x` `0xff` bytes.
pub fn nulls(n: usize) -> FieldValue {
    FieldValue::Bytes(vec![0xffu8; n])
}

/// `NULL` in `header.py`: the sentinel "no such record" index.
pub const NULL: u64 = 0xffff_ffff;

/// One field's static definition: name, default value, and whether an
/// `Int` value packs as a 2-byte (`SHORT_FIELDS` in Python) rather than
/// 4-byte big-endian integer.
#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: &'static str,
    pub default: FieldValue,
    pub short: bool,
}

impl FieldDef {
    pub fn new(name: &'static str, default: FieldValue) -> Self {
        FieldDef {
            name,
            default,
            short: false,
        }
    }

    pub fn short(name: &'static str, default: FieldValue) -> Self {
        FieldDef {
            name,
            default,
            short: true,
        }
    }
}

/// Port of the `Header` base class: an ordered set of named fields that
/// serializes to a flat binary buffer (`HEADER_NAME` + each field's
/// packed value, in definition order), with support for "position"
/// fields (`POSITIONS` in Python) that get back-patched with the byte
/// offset of another named field once the whole buffer is known.
pub struct Header {
    header_name: &'static [u8],
    align: bool,
    /// (name, value, short) in definition order.
    fields: Vec<(String, FieldValue, bool)>,
    /// pos_field -> field: after the main buffer is written, the 4 bytes
    /// at `pos_field`'s own offset get overwritten with `field`'s offset.
    positions: Vec<(&'static str, &'static str)>,
}

impl Header {
    pub fn new(
        header_name: &'static [u8],
        align: bool,
        defs: &[FieldDef],
        positions: &[(&'static str, &'static str)],
    ) -> Self {
        Header {
            header_name,
            align,
            fields: defs
                .iter()
                .map(|d| (d.name.to_string(), d.default.clone(), d.short))
                .collect(),
            positions: positions.to_vec(),
        }
    }

    /// Set field `name` to `val`. Port of the keyword arguments to
    /// `Header.__call__`. Errors (rather than panicking) on an unknown
    /// field name, matching Python's `KeyError` on the same.
    pub fn set(&mut self, name: &str, val: FieldValue) -> Result<&mut Self> {
        match self.fields.iter_mut().find(|(n, _, _)| n == name) {
            Some(f) => {
                f.1 = val;
                Ok(self)
            }
            None => bail!("Not a valid header field: {name:?}"),
        }
    }

    /// Build the header bytes. Port of `Header.__call__`. Errors if any
    /// field is still `FieldValue::Dyn` (Python's `ValueError('Dynamic
    /// field ... not set')`) or if a `POSITIONS` entry names a field
    /// that isn't in this header.
    pub fn build(&self) -> Result<Vec<u8>> {
        let mut buf = self.header_name.to_vec();
        let mut positions: HashMap<&str, usize> = HashMap::new();

        for (name, val, short) in &self.fields {
            positions.insert(name.as_str(), buf.len());
            match val {
                FieldValue::Dyn => bail!("Dynamic field {name:?} not set"),
                FieldValue::Bytes(b) => buf.extend_from_slice(b),
                FieldValue::Int(n) => {
                    if *short {
                        buf.extend_from_slice(&(*n as u16).to_be_bytes());
                    } else {
                        buf.extend_from_slice(&(*n as u32).to_be_bytes());
                    }
                }
            }
        }

        for (pos_field, field) in &self.positions {
            let field_pos = *positions
                .get(field)
                .ok_or_else(|| anyhow::anyhow!("unknown position-target field {field:?}"))?;
            let patch_at = *positions
                .get(pos_field)
                .ok_or_else(|| anyhow::anyhow!("unknown position field {pos_field:?}"))?;
            if patch_at + 4 > buf.len() {
                bail!("position field {pos_field:?} too small to hold an offset");
            }
            buf[patch_at..patch_at + 4].copy_from_slice(&(field_pos as u32).to_be_bytes());
        }

        if self.align {
            buf = align_block(&buf, 4, 0);
        }
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_simple_header_with_a_position_patch() {
        let defs = vec![
            FieldDef::new("ident", FieldValue::Bytes(b"TEST".to_vec())),
            FieldDef::new("ptr", FieldValue::Int(0)),
            FieldDef::new("count", FieldValue::Dyn),
            FieldDef::new("payload", FieldValue::Dyn),
        ];
        let mut h = Header::new(b"HDR", false, &defs, &[("ptr", "payload")]);
        h.set("count", FieldValue::Int(3)).unwrap();
        h.set("payload", FieldValue::Bytes(b"xyz".to_vec()))
            .unwrap();
        let bytes = h.build().unwrap();
        assert_eq!(&bytes[0..3], b"HDR");
        assert_eq!(&bytes[3..7], b"TEST");
        // `payload` starts right after ident(4) + ptr(4) + count(4) = 3(HDR)+4+4+4+4=19
        let ptr_val = u32::from_be_bytes(bytes[7..11].try_into().unwrap());
        assert_eq!(ptr_val as usize, 3 + 4 + 4 + 4);
        assert_eq!(&bytes[ptr_val as usize..ptr_val as usize + 3], b"xyz");
    }

    #[test]
    fn errors_on_unset_dynamic_field() {
        let defs = vec![FieldDef::new("x", FieldValue::Dyn)];
        let h = Header::new(b"H", false, &defs, &[]);
        assert!(h.build().is_err());
    }

    #[test]
    fn errors_on_unknown_field_name() {
        let defs = vec![FieldDef::new("x", FieldValue::Int(0))];
        let mut h = Header::new(b"H", false, &defs, &[]);
        assert!(h.set("y", FieldValue::Int(1)).is_err());
    }

    #[test]
    fn short_fields_pack_as_two_bytes() {
        let defs = vec![FieldDef::short("x", FieldValue::Int(7))];
        let h = Header::new(b"", false, &defs, &[]);
        let bytes = h.build().unwrap();
        assert_eq!(bytes.len(), 2);
        assert_eq!(u16::from_be_bytes(bytes.try_into().unwrap()), 7);
    }

    #[test]
    fn align_block_pads_to_a_multiple_of_four() {
        let defs = vec![FieldDef::new("x", FieldValue::Bytes(vec![1, 2, 3]))];
        let h = Header::new(b"", true, &defs, &[]);
        let bytes = h.build().unwrap();
        assert_eq!(bytes.len() % 4, 0);
    }
}
