//! Port of `old_src/src/calibre/ebooks/unihandecode/krdecoder.py`'s
//! `Krdecoder`. In Python this only overrides `Unidecoder.__init__`'s
//! `codepoints` table (Korean overrides instead of Chinese); `decode`
//! and everything it calls are inherited unchanged, so this reuses
//! `unidecoder`'s shared table-lookup machinery rather than
//! reimplementing it -- see that module's docs for the algorithm.

use crate::unihandecode::unidecoder::{decode_with_table, merge_tables, BlockMap};
use crate::unihandecode::{krcodepoints, unicodepoints};

pub struct Krdecoder {
    map: BlockMap,
}

impl Krdecoder {
    pub fn new() -> Self {
        Self {
            map: merge_tables(unicodepoints::CODEPOINTS, krcodepoints::CODEPOINTS),
        }
    }

    pub fn decode(&self, text: &str) -> String {
        decode_with_table(&self.map, text)
    }
}

impl Default for Krdecoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_passes_through_unchanged() {
        let d = Krdecoder::new();
        assert_eq!(d.decode("Hello, World!"), "Hello, World!");
    }

    #[test]
    fn hangul_syllable_uses_the_kr_override_table() {
        let d = Krdecoder::new();
        // U+AC00 (Hangul syllable "ga") -- krcodepoints overrides
        // unicodepoints' xac block, which has no entry for Hangul.
        assert_eq!(d.decode("\u{ac00}"), "ga");
    }

    #[test]
    fn unmapped_codepoint_falls_back_to_question_mark() {
        let d = Krdecoder::new();
        assert_eq!(d.decode("a\u{e000}b"), "a?b");
    }
}
