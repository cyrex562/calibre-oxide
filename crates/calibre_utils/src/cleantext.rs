//! Port of `old_src/src/calibre/utils/cleantext.py`: stripping
//! control characters and invalid-XML codepoints from text, and
//! decoding (or removing) HTML/XML character references and named
//! entities.
//!
//! # Scope
//!
//! Real: [`clean_ascii_chars`] and [`clean_xml_chars`] (upstream's own
//! pure-Python `py_clean_xml_chars` fallback -- there's no C
//! `speedup` extension to fast-path here, and this is simple enough
//! that it doesn't need one) and [`unescape`], backed by the
//! `htmlentity` crate for named/numeric entity decoding instead of
//! Python's `html.entities.name2codepoint` table.

use std::sync::OnceLock;

use regex::Regex;

/// Port of `clean_ascii_chars`: removes every ASCII control character
/// except `\t`/`\n`/`\r`.
pub fn clean_ascii_chars(text: &str) -> String {
    text.chars().filter(|&c| !is_stripped_control_char(c)).collect()
}

fn is_stripped_control_char(c: char) -> bool {
    let code = c as u32;
    (code < 32 && c != '\t' && c != '\n' && c != '\r') || code == 127
}

/// Port of `py_clean_xml_chars`'s `allowed` predicate: whether a
/// codepoint is valid in an XML 1.0 document.
fn is_valid_xml_char(c: char) -> bool {
    let x = c as u32;
    (x != 127 && (31 < x && x < 0xd7ff || matches!(x, 9 | 10 | 13))) || (0xe000 < x && x < 0xfffd) || (0x10000 < x && x < 0x10ffff)
}

/// Port of `clean_xml_chars` (the `py_clean_xml_chars` path -- see
/// the module doc for why there's no separate "native" fast path).
pub fn clean_xml_chars(text: &str) -> String {
    text.chars().filter(|&c| is_valid_xml_char(c)).collect()
}

fn entity_ref_pattern() -> &'static Regex {
    static PAT: OnceLock<Regex> = OnceLock::new();
    PAT.get_or_init(|| Regex::new(r"&#?\w+;").unwrap())
}

/// Port of `unescape`: decodes HTML/XML character references
/// (`&#65;`/`&#x41;`) and named entities (`&amp;`) in `text`. When
/// `rm` is `true`, a *recognized* reference is replaced with `rchar`
/// instead of its decoded character (used to strip markup-derived
/// entities down to a placeholder). An unrecognized reference is left
/// exactly as written, matching upstream's own fallback.
pub fn unescape(text: &str, rm: bool, rchar: &str) -> String {
    entity_ref_pattern()
        .replace_all(text, |caps: &regex::Captures| {
            let m = &caps[0];
            let chars: Vec<char> = m.chars().collect();
            let decoded = htmlentity::entity::decode_chars(&chars);
            if decoded.len() == 1 {
                if rm { rchar.to_string() } else { decoded[0].to_string() }
            } else {
                m.to_string()
            }
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_control_chars_but_keeps_tab_newline_cr() {
        assert_eq!(clean_ascii_chars("a\x02b\tc\nd\re\x7f"), "ab\tc\nd\re");
    }

    #[test]
    fn clean_ascii_chars_of_empty_string_is_empty() {
        assert_eq!(clean_ascii_chars(""), "");
    }

    #[test]
    fn clean_xml_chars_matches_the_documented_example() {
        // The exact case from upstream's own test_clean_xml_chars.
        let raw = "asd\u{2}a\u{10437}x\u{fffe}b";
        assert_eq!(clean_xml_chars(raw), "asda\u{10437}xb");
    }

    #[test]
    fn unescape_decodes_named_and_numeric_references() {
        assert_eq!(unescape("Tom &amp; Jerry", false, ""), "Tom & Jerry");
        assert_eq!(unescape("&#65;&#x42;", false, ""), "AB");
        assert_eq!(unescape("&nbsp;", false, ""), "\u{a0}");
    }

    #[test]
    fn unescape_leaves_unrecognized_references_alone() {
        assert_eq!(unescape("&notarealentity;", false, ""), "&notarealentity;");
    }

    #[test]
    fn unescape_with_rm_replaces_recognized_references_with_rchar() {
        assert_eq!(unescape("a &amp; b", true, " "), "a   b");
        assert_eq!(unescape("a &notreal; b", true, " "), "a &notreal; b");
    }
}
