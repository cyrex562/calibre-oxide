//! Port of `old_src/src/calibre/ebooks/rtf2xml/char_set.py`.
//!
//! The Python module is a single module-level string constant,
//! `char_set`, holding ~16,700 lines of character/codepage/font-symbol
//! lookup data. It defines no logic at all -- parsing it into per-table
//! maps is [`super::get_char_map`]'s job (`GetCharMap.get_char_map`),
//! invoked at runtime by downstream rtf2xml passes (`hex_2_utf8.py`,
//! out of scope here -- see the crate-level module docs).
//!
//! # Data format
//!
//! The string is organized into named sections, each delimited by a
//! `<name>` / `</name>` line pair (e.g. `<ms_standard>` ...
//! `</ms_standard>`, `<ansicpg1252>` ... `</ansicpg1252>`,
//! `<SYMBOL>` ... `</SYMBOL>`). Between the tags, each non-blank line is
//! a `:`-delimited record:
//!
//! ```text
//! UNICODE NAME:rtf-or-source-key:decimal-codepoint:replacement-text
//! ```
//!
//! for example `LEFT DOUBLE QUOTATION MARK:ldblquote:8220:&#x201C;`.
//! [`super::get_char_map::GetCharMap`] only ever reads field index 1
//! (the key) and field index 3 (the replacement text) -- see that
//! module for the exact algorithm, including a preserved upstream quirk
//! where a source-level `\` + newline continuation merges two records
//! in the `bottom_128` table.
//!
//! # Porting approach
//!
//! This ~700KB table is overwhelmingly *data*, not logic: hand
//! transcribing 16,700 lines risks silent transcription errors with no
//! way to catch them. Instead, `char_set_data.txt` (next to this file)
//! is a byte-for-byte extraction of the Python `char_set` string's
//! *evaluated* value -- produced by actually importing `char_set.py`
//! with the standard library and writing `char_set` back out as UTF-8,
//! not by copying the raw source text. This matters because the raw
//! source contains Python string-literal escapes that change the
//! evaluated content, e.g. `\'` unescaping to `'` (148 occurrences) and
//! -- consequentially -- a literal `\` immediately followed by a
//! newline inside the `bottom_128` table (the "REVERSE SOLIDUS" entry's
//! replacement column, meant to be a literal backslash) which Python
//! treats as a line-continuation escape and silently splices the
//! following line onto it. That splice is a genuine upstream data bug
//! (it corrupts the "REVERSE SOLIDUS" record and swallows the
//! following "RIGHT SQUARE BRACKET" record entirely -- see
//! `super::get_char_map` tests for the verified fallout), and this
//! extraction approach reproduces it faithfully rather than
//! "fixing" it, matching this table byte-for-byte with what
//! `from calibre.ebooks.rtf2xml.char_set import char_set` actually
//! returns at runtime.
//!
//! Spot-checked by hand against the Python source across every distinct
//! sub-table family (ms_standard/ms_symbol/ms_dingbats/ms_wingdings,
//! not_unicode/bottom_128 control chars, SYMBOL and wingdings/dingbats
//! font maps, and several `ansicpgNNNN` codepages) in
//! `super::get_char_map`'s unit tests below.

/// The raw, unparsed character-set table, byte-for-byte identical to
/// the Python `char_set` module-level string. Callers almost always
/// want [`super::get_char_map::GetCharMap`] instead of parsing this
/// directly.
pub const CHAR_SET: &str = include_str!("char_set_data.txt");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_expected_section_tags() {
        for tag in [
            "<ms_standard>",
            "</ms_standard>",
            "<ms_symbol>",
            "<ms_dingbats>",
            "<ms_wingdings>",
            "<not_unicode>",
            "<bottom_128>",
            "<ansicpg1252>",
            "<SYMBOL>",
            "<wingdings>",
            "<dingbats>",
            "<caps_uni>",
        ] {
            assert!(CHAR_SET.contains(tag), "missing section tag {tag}");
        }
    }

    #[test]
    fn matches_expected_length_and_line_count() {
        // Verified against a live `import char_set; len(char_set.char_set)`
        // run of the Python module: 705630 Unicode *characters*
        // (Python's `len()` on a `str`). The table contains exactly
        // one non-ASCII character (U+00EF, 2 UTF-8 bytes), so the
        // Rust `&str`'s *byte* length (what `str::len()` returns) is
        // one higher, 705631.
        assert_eq!(CHAR_SET.chars().count(), 705630);
        assert_eq!(CHAR_SET.len(), 705631);
        assert_eq!(CHAR_SET.lines().count(), 16706);
    }

    #[test]
    fn spot_check_a_known_record_verbatim() {
        assert!(CHAR_SET.contains("LEFT DOUBLE QUOTATION MARK:ldblquote:8220:&#x201C;"));
        assert!(CHAR_SET.contains("REGISTERED SIGN:ldblquote:174:&#x00AE;"));
    }
}
