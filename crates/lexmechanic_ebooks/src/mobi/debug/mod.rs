//! Debug/introspection tooling for MOBI files.
//!
//! Port of `src/calibre/ebooks/mobi/debug/`. This is calibre's
//! `calibre-debug -m book.mobi` support: parse a MOBI/AZW3 file down
//! to its raw structural pieces (PalmDB header, record table, MOBI/EXTH
//! headers, indices, text/image/font records) and dump each as a
//! separate file under a directory, for developers diagnosing a
//! malformed or unusual book.
//!
//! It deliberately does *not* reuse `crate::mobi::{headers, index,
//! mobi6}` (the production reader): those modules are shaped around
//! producing a clean [`crate::oeb::book::OEBBook`], while this one's
//! entire job is showing every byte, including the ones the reader
//! throws away. The two do share the genuinely low-level,
//! format-agnostic pieces — binary-integer decoding, the `TAGX`/`INDX`
//! record grammar, font de-obfuscation — via `pub(crate)` helpers on
//! `crate::mobi::index` and public ones on `crate::mobi::{langcodes,
//! utils}`.

pub mod containers;
pub mod headers;
pub mod index;
pub mod main;
pub mod mobi6;
pub mod mobi8;

/// `format_bytes` in `mobi/debug/__init__.py` — a lowercase
/// space-separated hex dump, used throughout this module for "here are
/// the raw bytes" fields.
pub fn format_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_matches_the_pythons_lowercase_hex_join() {
        assert_eq!(format_bytes(&[0x00, 0xff, 0x1a]), "0 ff 1a");
        assert_eq!(format_bytes(&[]), "");
    }
}
