//! Port of `old_src/src/calibre/ebooks/unihandecode/vndecoder.py`'s
//! `Vndecoder` -- see `unidecoder.rs`'s docs for the shared algorithm;
//! this only supplies a different override table (Vietnamese instead
//! of Chinese), exactly like Python's `Vndecoder(Unidecoder)` overrides
//! nothing but `__init__`'s `codepoints` dict.

use crate::unihandecode::unidecoder::{decode_with_table, merge_tables, BlockMap};
use crate::unihandecode::{unicodepoints, vncodepoints};

pub struct Vndecoder {
    map: BlockMap,
}

impl Vndecoder {
    pub fn new() -> Self {
        Self {
            map: merge_tables(unicodepoints::CODEPOINTS, vncodepoints::CODEPOINTS),
        }
    }

    pub fn decode(&self, text: &str) -> String {
        decode_with_table(&self.map, text)
    }
}

impl Default for Vndecoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_passes_through_unchanged() {
        let d = Vndecoder::new();
        assert_eq!(d.decode("Hello, World!"), "Hello, World!");
    }

    #[test]
    fn vietnamese_diacritic_uses_the_vn_override_table() {
        let d = Vndecoder::new();
        // U+1EA1 (LATIN SMALL LETTER A WITH DOT BELOW, "a." in Vietnamese).
        assert_eq!(d.decode("\u{1ea1}"), "a");
    }

    #[test]
    fn unmapped_codepoint_falls_back_to_question_mark() {
        let d = Vndecoder::new();
        assert_eq!(d.decode("a\u{e000}b"), "a?b");
    }
}
