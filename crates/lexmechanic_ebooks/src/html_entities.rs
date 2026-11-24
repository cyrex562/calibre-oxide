//! HTML entity decoding + encoding.
//!
//! Port of `old_src/src/calibre/ebooks/html_entities.{py,c,h}`. The
//! Python original hand-maintained a 5000-line lookup table plus a C
//! extension that consumed it. Rust delegates to the well-maintained
//! `html-escape` crate, which uses the WHATWG HTML5 entity list.
//!
//! `html-escape` covers:
//! - Named entities: `&amp;`, `&nbsp;`, `&mdash;`, ...
//! - Numeric decimal entities: `&#8212;`.
//! - Numeric hex entities: `&#x2014;`.
//!
//! We wrap it in this thin module so any future switch (e.g., to a
//! Calibre-specific fork that treats malformed entities differently)
//! has one place to change.

/// Decode HTML/XML entities in a UTF-8 string. Unknown entities are
/// left unchanged (matching the WHATWG HTML5 parsing rule the C
/// original followed).
pub fn decode_entities(s: &str) -> String {
    html_escape::decode_html_entities(s).into_owned()
}

/// Encode a string for safe HTML output: `&`, `<`, `>`, `"`, `'`
/// become their entity forms.
pub fn encode_entities(s: &str) -> String {
    html_escape::encode_safe(s).into_owned()
}

/// The Python module also exposed `xml_replace_entities` — same as
/// `decode_entities` but historically only for XML-permitted names.
/// Since we're delegating to a full HTML5 table, the two collapse to
/// the same function; alias for source compatibility.
pub fn xml_replace_entities(s: &str) -> String {
    decode_entities(s)
}

/// Byte-oriented variants for callers working on `Vec<u8>` (rare but
/// used by parts of the MOBI pipeline).
pub fn decode_entities_bytes(raw: &[u8]) -> Vec<u8> {
    let s = std::str::from_utf8(raw).unwrap_or("");
    if s.is_empty() {
        return raw.to_vec();
    }
    decode_entities(s).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_named_entities() {
        assert_eq!(decode_entities("&amp;"), "&");
        assert_eq!(decode_entities("&nbsp;"), "\u{a0}");
        assert_eq!(decode_entities("&mdash;"), "—");
        assert_eq!(decode_entities("A &amp; B &lt; C"), "A & B < C");
    }

    #[test]
    fn decode_numeric_entities_decimal_and_hex() {
        assert_eq!(decode_entities("&#8212;"), "—");
        assert_eq!(decode_entities("&#x2014;"), "—");
        assert_eq!(decode_entities("&#65;"), "A");
    }

    #[test]
    fn decode_unknown_entity_left_unchanged() {
        // The WHATWG parsing rule: unknown named entities stay
        // literal. `html-escape` follows this.
        assert_eq!(decode_entities("&notarealentity;"), "&notarealentity;");
    }

    #[test]
    fn decode_no_entities_is_identity() {
        assert_eq!(decode_entities("hello world"), "hello world");
    }

    #[test]
    fn encode_entities_escapes_five_chars() {
        // encode_safe covers &, <, >, ", '
        assert_eq!(encode_entities("A & B"), "A &amp; B");
        assert_eq!(encode_entities("<p>"), "&lt;p&gt;");
        assert_eq!(encode_entities("\"hi\""), "&quot;hi&quot;");
    }

    #[test]
    fn encode_decode_roundtrip() {
        let original = "A & B < C > D \"quoted\" 'apos'";
        let encoded = encode_entities(original);
        let decoded = decode_entities(&encoded);
        assert_eq!(decoded, original);
    }

    #[test]
    fn xml_replace_entities_matches_decode() {
        // Aliased for source-compat with Python callers.
        assert_eq!(xml_replace_entities("&amp;"), decode_entities("&amp;"));
    }

    #[test]
    fn decode_bytes_matches_str_variant() {
        assert_eq!(
            decode_entities_bytes(b"A &amp; B"),
            decode_entities("A &amp; B").into_bytes()
        );
    }

    #[test]
    fn decode_bytes_invalid_utf8_returns_unchanged() {
        // Invalid UTF-8 input: return the bytes unchanged rather
        // than panicking. Defensive matches the Python C extension
        // which would just fall through.
        let raw = &[0xFF, 0xFE, 0xFD][..];
        assert_eq!(decode_entities_bytes(raw), raw);
    }
}
