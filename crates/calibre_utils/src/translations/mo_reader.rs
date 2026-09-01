//! A minimal reader for the binary GNU `.mo` catalog format that
//! [`super::msgfmt`] produces (and that upstream calibre's own
//! `gettext`-based runtime translation machinery consumes). There is
//! no single upstream Python file this corresponds to 1:1 -- CPython's
//! own `gettext.GNUTranslations` plays this role for calibre at
//! runtime -- but a reader is needed in this crate both to give
//! [`super::dynamic`] something real to look translations up in, and
//! to round-trip-test [`super::msgfmt::compile_po`]'s output.
//!
//! # Scope
//!
//! Real: the full binary layout (header, sorted key/value offset
//! tables, null-terminated string pools), `msgctxt` lookup (composite
//! `ctxt + \x04 + msgid` key, matching `msgfmt.py`'s own encoding),
//! and `msgid_plural` lookup.
//!
//! Narrowed: CPython's `GNUTranslations` parses a `Plural-Forms: ...`
//! C-expression out of the header entry and evaluates it per-locale to
//! pick a plural index. This project doesn't vendor a C-expression
//! evaluator, and no locale data is vendored either (see
//! [`super::dynamic`]'s own doc) -- so [`Catalog::ngettext`] always
//! uses the English rule (`n == 1` selects plural form 0, anything
//! else selects form 1), disclosed here rather than silently wrong for
//! languages with richer plural rules.

use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum MoReadError {
    #[error("not a valid .mo file: bad magic number")]
    BadMagic,
    #[error("not a valid .mo file: truncated")]
    Truncated,
    #[error("unsupported .mo format version")]
    UnsupportedVersion,
}

/// A parsed `.mo` catalog: msgid (or `ctxt\x04msgid`) -> msgstr,
/// as raw bytes (matching upstream's own byte-level, not necessarily
/// UTF-8-only, treatment -- see [`Catalog::gettext`] for the UTF-8-lossy
/// string-returning convenience layer on top).
pub struct Catalog {
    messages: HashMap<Vec<u8>, Vec<u8>>,
}

fn read_u32_le(data: &[u8], offset: usize) -> Result<u32, MoReadError> {
    data.get(offset..offset + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or(MoReadError::Truncated)
}

impl Catalog {
    /// Parses raw `.mo` file bytes into a lookup table.
    pub fn parse(data: &[u8]) -> Result<Catalog, MoReadError> {
        let magic = read_u32_le(data, 0)?;
        if magic != 0x950412de {
            return Err(MoReadError::BadMagic);
        }
        let version = read_u32_le(data, 4)?;
        if version != 0 {
            return Err(MoReadError::UnsupportedVersion);
        }
        let num_strings = read_u32_le(data, 8)? as usize;
        let key_index_offset = read_u32_le(data, 12)? as usize;
        let value_index_offset = read_u32_le(data, 16)? as usize;

        let mut messages = HashMap::with_capacity(num_strings);
        for i in 0..num_strings {
            let klen = read_u32_le(data, key_index_offset + i * 8)? as usize;
            let koff = read_u32_le(data, key_index_offset + i * 8 + 4)? as usize;
            let vlen = read_u32_le(data, value_index_offset + i * 8)? as usize;
            let voff = read_u32_le(data, value_index_offset + i * 8 + 4)? as usize;

            let key = data.get(koff..koff + klen).ok_or(MoReadError::Truncated)?.to_vec();
            let value = data.get(voff..voff + vlen).ok_or(MoReadError::Truncated)?.to_vec();
            messages.insert(key, value);
        }

        Ok(Catalog { messages })
    }

    /// Context-free lookup, e.g. `catalog.gettext("Hello")`. Returns
    /// `None` if untranslated (matching upstream's convention of
    /// falling back to the original msgid at the call site, not
    /// inside the catalog itself).
    pub fn gettext(&self, msgid: &str) -> Option<String> {
        self.messages.get(msgid.as_bytes()).map(|v| String::from_utf8_lossy(v).into_owned())
    }

    /// Context-scoped lookup (`msgctxt`), e.g.
    /// `catalog.pgettext("menu", "Open")`.
    pub fn pgettext(&self, ctxt: &str, msgid: &str) -> Option<String> {
        let mut key = ctxt.as_bytes().to_vec();
        key.push(0x04);
        key.extend_from_slice(msgid.as_bytes());
        self.messages.get(&key).map(|v| String::from_utf8_lossy(v).into_owned())
    }

    /// Plural-form lookup. See this module's own doc for the English-
    /// only plural-rule narrowing.
    pub fn ngettext(&self, msgid: &str, msgid_plural: &str, n: u64) -> Option<String> {
        let mut key = msgid.as_bytes().to_vec();
        key.push(0);
        key.extend_from_slice(msgid_plural.as_bytes());
        let value = self.messages.get(&key)?;
        let index = if n == 1 { 0 } else { 1 };
        value.split(|&b| b == 0).nth(index).map(|s| String::from_utf8_lossy(s).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::translations::msgfmt::compile_po;

    #[test]
    fn round_trips_simple_translations() {
        let po = "msgid \"Hello\"\nmsgstr \"Bonjour\"\n\nmsgid \"Goodbye\"\nmsgstr \"Au revoir\"\n";
        let (mo, _stats) = compile_po(po, false).unwrap();
        let catalog = Catalog::parse(&mo).unwrap();
        assert_eq!(catalog.gettext("Hello"), Some("Bonjour".to_string()));
        assert_eq!(catalog.gettext("Goodbye"), Some("Au revoir".to_string()));
        assert_eq!(catalog.gettext("Missing"), None);
    }

    #[test]
    fn round_trips_msgctxt() {
        let po = "msgctxt \"menu\"\nmsgid \"Open\"\nmsgstr \"Ouvrir\"\n";
        let (mo, _stats) = compile_po(po, false).unwrap();
        let catalog = Catalog::parse(&mo).unwrap();
        assert_eq!(catalog.pgettext("menu", "Open"), Some("Ouvrir".to_string()));
        assert_eq!(catalog.gettext("Open"), None);
    }

    #[test]
    fn round_trips_plural_forms() {
        let po = "msgid \"one book\"\nmsgid_plural \"%d books\"\nmsgstr[0] \"un livre\"\nmsgstr[1] \"%d livres\"\n";
        let (mo, _stats) = compile_po(po, false).unwrap();
        let catalog = Catalog::parse(&mo).unwrap();
        assert_eq!(catalog.ngettext("one book", "%d books", 1), Some("un livre".to_string()));
        assert_eq!(catalog.ngettext("one book", "%d books", 5), Some("%d livres".to_string()));
    }

    #[test]
    fn rejects_bad_magic() {
        assert!(matches!(Catalog::parse(&[0, 0, 0, 0]), Err(MoReadError::BadMagic)));
    }
}
