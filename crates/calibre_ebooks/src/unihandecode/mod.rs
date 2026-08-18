//! Port of `old_src/src/calibre/ebooks/unihandecode/` (issue #55):
//! Unicode-to-ASCII transliteration ("Unicode handle-decode"), used to
//! produce a readable ASCII fallback for non-ASCII text (e.g. deriving
//! an ASCII filename/sort-key from a CJK or accented title).
//!
//! - `unicodepoints.rs`/`zhcodepoints.rs`/`krcodepoints.rs`/
//!   `vncodepoints.rs`/`jacodepoints.rs`: the codepoint replacement
//!   tables themselves, generated directly from the live `CODEPOINTS`
//!   dict in each same-named `.py` file (not hand-transcribed) --
//!   see each file's header for how.
//! - `unidecoder.rs`: the shared block/index table-lookup algorithm
//!   (`Unidecoder`, the "zh and others" default), reused by the other
//!   three decoders below.
//! - `krdecoder.rs`/`vndecoder.rs`/`jadecoder.rs`: port of
//!   `krdecoder.py`/`vndecoder.py`/`jadecoder.py`. The first two are
//!   `unidecoder`'s algorithm over a different override table, exactly
//!   like their Python originals (which only override `__init__`).
//!   `jadecoder.py` also wraps a full pykakasi-based Japanese analysis
//!   pass Python runs before the table fallback -- not ported, see
//!   `jadecoder.rs`'s docs for why and what that costs in output
//!   quality.
//!
//! This module's own `Unihandecoder` (below) is the port of
//! `__init__.py`'s dispatcher. One simplification versus Python: the
//! Python constructor also takes an `encoding` used to decode raw
//! `bytes` input before processing. This crate's converters hand text
//! around as `&str` (already-decoded) throughout, so there is no
//! bytes-input path for that parameter to apply to; it's dropped
//! rather than kept as unused API surface.

pub mod jacodepoints;
pub mod jadecoder;
pub mod krcodepoints;
pub mod krdecoder;
pub mod unicodepoints;
pub mod unidecoder;
pub mod vncodepoints;
pub mod vndecoder;
pub mod zhcodepoints;

use unicode_normalization::UnicodeNormalization;

use jadecoder::Jadecoder;
use krdecoder::Krdecoder;
use unidecoder::Unidecoder;
use vndecoder::Vndecoder;

enum LangDecoder {
    Ja(Jadecoder),
    Kr(Krdecoder),
    Vn(Vndecoder),
    Other(Unidecoder),
}

/// Port of `Unihandecoder`: picks a language-specific decoder by the
/// two-letter (or full-name) language prefix, exactly matching
/// `__init__.py`'s dispatch (`lang[:2] == 'ja'`, `lang[:2] == 'kr' or
/// lang == 'korean'`, `lang[:2] == 'vn' or lang == 'vietnum'`, else
/// Chinese/default).
pub struct Unihandecoder {
    decoder: LangDecoder,
}

impl Unihandecoder {
    pub fn new(lang: &str) -> Self {
        let lang = lang.to_lowercase();
        let prefix = lang.get(..2).unwrap_or(&lang);
        let decoder = if prefix == "ja" {
            LangDecoder::Ja(Jadecoder::new())
        } else if prefix == "kr" || lang == "korean" {
            LangDecoder::Kr(Krdecoder::new())
        } else if prefix == "vn" || lang == "vietnum" {
            LangDecoder::Vn(Vndecoder::new())
        } else {
            LangDecoder::Other(Unidecoder::new())
        };
        Self { decoder }
    }

    /// Port of `decode`: NFKC-normalize, then transliterate every
    /// non-ASCII character via the selected language's decoder.
    pub fn decode(&self, text: &str) -> String {
        let normalized: String = text.nfkc().collect();
        match &self.decoder {
            LangDecoder::Ja(d) => d.decode(&normalized),
            LangDecoder::Kr(d) => d.decode(&normalized),
            LangDecoder::Vn(d) => d.decode(&normalized),
            LangDecoder::Other(d) => d.decode(&normalized),
        }
    }
}

impl Default for Unihandecoder {
    /// Port of `lang='zh'`, `__init__.py`'s default.
    fn default() -> Self {
        Self::new("zh")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_the_chinese_zh_decoder() {
        let d = Unihandecoder::default();
        assert_eq!(d.decode("\u{4e00}"), "Yi ");
    }

    #[test]
    fn dispatches_to_japanese_by_prefix() {
        let d = Unihandecoder::new("ja");
        assert_eq!(d.decode("\u{4e00}"), "Ichi ");
    }

    #[test]
    fn dispatches_to_korean_by_two_letter_prefix_or_full_name() {
        assert_eq!(Unihandecoder::new("kr").decode("\u{ac00}"), "ga");
        assert_eq!(Unihandecoder::new("korean").decode("\u{ac00}"), "ga");
    }

    #[test]
    fn dispatches_to_vietnamese_by_two_letter_prefix_or_full_name() {
        assert_eq!(Unihandecoder::new("vn").decode("\u{1ea1}"), "a");
        assert_eq!(Unihandecoder::new("vietnum").decode("\u{1ea1}"), "a");
    }

    #[test]
    fn language_matching_is_case_insensitive() {
        assert_eq!(Unihandecoder::new("JA").decode("\u{4e00}"), "Ichi ");
    }

    #[test]
    fn unrecognized_language_falls_back_to_the_default_zh_decoder() {
        let d = Unihandecoder::new("xx");
        assert_eq!(d.decode("\u{4e00}"), "Yi ");
    }

    #[test]
    fn nfkc_normalizes_before_transliterating() {
        let d = Unihandecoder::default();
        // U+FF21 FULLWIDTH LATIN CAPITAL LETTER A NFKC-normalizes to
        // ASCII 'A' before ever reaching the codepoint table.
        assert_eq!(d.decode("\u{ff21}"), "A");
    }
}
