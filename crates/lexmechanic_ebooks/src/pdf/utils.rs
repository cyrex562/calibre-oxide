//! Port of `old_src/src/calibre/ebooks/pdf/utils.h`.
//!
//! The original is a tiny C++ header meant to back a (never wired up by
//! `reflow.py`, which is pure Python + `lxml`) native speed-optimized
//! companion to the reflow logic: an XML-escaping helper and an exception
//! type. `reflow.rs` reuses [`encode_for_xml`] for attribute-value/text
//! escaping when re-serializing `pdftohtml -xml` fragments back to markup.

use std::fmt;

/// Port of `calibre_reflow::encode_for_xml` (`utils.h`, lines 22-42).
///
/// Escapes the four characters that must not appear unescaped in XML
/// character data or double-quoted attribute values: `&`, `<`, `>`, `"`.
/// Note this intentionally does **not** escape `'` (single quote) -
/// matching the C++ original, which only targets double-quoted XML
/// attributes/text, not HTML-style single-quoted attributes.
pub fn encode_for_xml(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for c in src.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Port of `calibre_reflow::ReflowException` (`utils.h`, lines 15-20).
///
/// The C++ original is a bare `std::exception` subclass carrying a
/// `const char *` message. Kept as a small standalone error type so any
/// site that needs "the reflow algorithm hit a fatal, non-recoverable
/// condition" (as opposed to a malformed-input parse error, which uses
/// [`super::reflow::ReflowError`]) has one to reach for.
#[derive(Debug, Clone)]
pub struct ReflowException {
    msg: String,
}

impl ReflowException {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }

    pub fn what(&self) -> &str {
        &self.msg
    }
}

impl fmt::Display for ReflowException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for ReflowException {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_the_four_xml_special_chars() {
        assert_eq!(
            encode_for_xml(r#"a & b < c > d "e" 'f'"#),
            r#"a &amp; b &lt; c &gt; d &quot;e&quot; 'f'"#
        );
    }

    #[test]
    fn leaves_plain_text_untouched() {
        assert_eq!(encode_for_xml("hello world"), "hello world");
    }

    #[test]
    fn reflow_exception_carries_message() {
        let e = ReflowException::new("boom");
        assert_eq!(e.what(), "boom");
        assert_eq!(e.to_string(), "boom");
    }
}
