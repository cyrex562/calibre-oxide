//! HTML parsing entry point.
//!
//! Port of `old_src/src/calibre/ebooks/BeautifulSoup.py` — a 35-line
//! shim over the BeautifulSoup Python library that also handled
//! encoding-declaration stripping and entity resolution before
//! parsing. Rust replacement uses `roxmltree` where the input is
//! well-formed XML and a lenient string preprocessor for
//! entity/encoding handling.
//!
//! Real HTML5 parsing (production quality — quirks-mode, tag-soup
//! recovery) belongs to `html5ever` / `scraper`; adding that is
//! larger than a stub replacement warrants. This module gives the
//! preprocessing pipeline callers need TODAY:
//!
//! 1. `strip_encoding_declarations` (via `chardet::strip_encoding_declarations`).
//! 2. `xml_replace_entities` (via `html_entities::xml_replace_entities`).
//! 3. Byte → String decoding for byte inputs (via `chardet::xml_to_unicode`).
//! 4. `clean_xml_chars` — strip control chars that break XML parsers.

use crate::chardet::{strip_encoding_declarations, xml_to_unicode};
use crate::html_entities::xml_replace_entities;

/// Prepare a UTF-8 HTML/XML string for downstream parsing:
///
/// 1. Strip any `<?xml ... encoding="...">` / `<meta charset=...>`
///    declarations from the first 50 KB — these confuse parsers when
///    the byte→str conversion already happened.
/// 2. Replace entity references (`&amp;`, `&#8212;`, ...).
/// 3. Remove XML-illegal control chars.
pub fn preprocess_html(input: &str) -> String {
    let stripped_bytes = strip_encoding_declarations(input.as_bytes(), 50 * 1024, false);
    let stripped = String::from_utf8_lossy(&stripped_bytes).into_owned();
    let with_entities_resolved = xml_replace_entities(&stripped);
    clean_xml_chars(&with_entities_resolved)
}

/// Same as [`preprocess_html`] but takes bytes and auto-detects the
/// encoding via [`crate::chardet::xml_to_unicode`]. Returns the
/// decoded, cleaned string plus the encoding it detected.
pub fn preprocess_html_bytes(input: &[u8]) -> (String, Option<String>) {
    let (decoded, enc) = xml_to_unicode(input, true, false);
    (
        {
            let with_entities = xml_replace_entities(&decoded);
            clean_xml_chars(&with_entities)
        },
        enc,
    )
}

/// Remove XML 1.0-illegal control characters. Preserves TAB (U+0009),
/// LF (U+000A), CR (U+000D). Everything else in the C0 control range
/// (and the isolated surrogate range U+D800..=U+DFFF) is dropped.
pub fn clean_xml_chars(s: &str) -> String {
    s.chars()
        .filter(|&c| is_xml_char(c))
        .collect()
}

fn is_xml_char(c: char) -> bool {
    let cp = c as u32;
    matches!(cp,
        0x09 | 0x0A | 0x0D
        | 0x20..=0xD7FF
        | 0xE000..=0xFFFD
        | 0x10000..=0x10FFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_strips_meta_charset() {
        let input = "<html><head><meta charset='utf-8'></head><body>x</body></html>";
        let out = preprocess_html(input);
        assert!(!out.contains("charset"), "{out:?}");
        assert!(out.contains("<body>x</body>"));
    }

    #[test]
    fn preprocess_resolves_entities() {
        let input = "A &amp; B &mdash; C";
        assert_eq!(preprocess_html(input), "A & B — C");
    }

    #[test]
    fn clean_xml_chars_drops_control_chars() {
        // NUL, BEL, VT — all illegal in XML 1.0.
        let input = "hello\x00\x07\x0bworld";
        assert_eq!(clean_xml_chars(input), "helloworld");
    }

    #[test]
    fn clean_xml_chars_preserves_tab_lf_cr() {
        let input = "line1\tstill line1\nline2\r\nline3";
        assert_eq!(clean_xml_chars(input), input);
    }

    #[test]
    fn preprocess_html_bytes_detects_utf8_bom() {
        let mut input = vec![0xEF, 0xBB, 0xBF];
        input.extend_from_slice(b"<p>hi</p>");
        let (out, enc) = preprocess_html_bytes(&input);
        assert_eq!(out, "<p>hi</p>");
        assert_eq!(enc.as_deref(), Some("utf-8"));
    }

    #[test]
    fn is_xml_char_boundary_cases() {
        assert!(is_xml_char('\t'));
        assert!(is_xml_char('\n'));
        assert!(is_xml_char('\r'));
        assert!(is_xml_char(' '));
        assert!(is_xml_char('a'));
        assert!(is_xml_char('中'));
        assert!(!is_xml_char('\x00'));
        assert!(!is_xml_char('\x07'));
        assert!(!is_xml_char('\x0b'));
        assert!(!is_xml_char('\u{FFFE}'));
        assert!(!is_xml_char('\u{FFFF}'));
    }
}
