//! Port of `old_src/src/calibre/ebooks/unihandecode/jadecoder.py`'s
//! `Jadecoder`.
//!
//! # What's ported: the codepoint-table fallback
//!
//! `Jadecoder(Unidecoder)` overrides `__init__`'s `codepoints` table
//! with `jacodepoints.CODEPOINTS` (Japanese-specific overrides layered
//! on `unicodepoints`, same whole-block-replace merge as
//! `Krdecoder`/`Vndecoder` -- see `unidecoder.rs`'s docs). That half is
//! a plain data-table port, done here exactly like the other three
//! decoders.
//!
//! # What's genuinely blocked: pykakasi
//!
//! Python's `Jadecoder.decode` first runs the *entire* input through
//! `pykakasi` -- a real Japanese morphological analyzer with bundled
//! Kanwa/Itaiji dictionaries that resolves kanji to their correct
//! on'yomi/kun'yomi readings in context and romanizes hiragana/katakana
//! via Hepburn -- and only applies the codepoint-table substitution to
//! whatever pykakasi leaves non-ASCII (normally nothing, since kakasi
//! covers the whole Japanese script repertoire it knows). There is no
//! Rust port of pykakasi or its dictionaries in this workspace, and
//! writing one is a project of its own, not a few-hundred-line addition
//! to this module -- genuinely blocked in the sense
//! `docs/AGENT_PORTING_GUIDE.md` means it.
//!
//! What this means for output quality: without kakasi's
//! dictionary-driven reading resolution, `decode` here falls straight
//! to the codepoint-table substitution for *every* non-ASCII character,
//! Kanji included. `jacodepoints` does have its own entries for common
//! Kanji (Japanese on'yomi-flavored, e.g. U+4E00 -> `"Ichi "` rather
//! than `zhcodepoints`' Mandarin pinyin `"Yi "` for the same character)
//! and for Hiragana/Katakana, so this still produces a real,
//! recognizable romanization -- just a per-character one, not
//! kakasi's context-aware, correctly-capitalized, word-boundary-aware
//! one. That gap is real and worth closing if pykakasi (or an
//! equivalent) ever gets ported, at which point this should call it
//! the way Python does before falling back to the table.

use crate::unihandecode::unidecoder::{decode_with_table, merge_tables, BlockMap};
use crate::unihandecode::{jacodepoints, unicodepoints};

pub struct Jadecoder {
    map: BlockMap,
}

impl Jadecoder {
    pub fn new() -> Self {
        Self {
            map: merge_tables(unicodepoints::CODEPOINTS, jacodepoints::CODEPOINTS),
        }
    }

    /// Codepoint-table transliteration only -- see the module docs for
    /// why the upstream kakasi-based pass isn't ported.
    pub fn decode(&self, text: &str) -> String {
        decode_with_table(&self.map, text)
    }
}

impl Default for Jadecoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_passes_through_unchanged() {
        let d = Jadecoder::new();
        assert_eq!(d.decode("Hello, World!"), "Hello, World!");
    }

    #[test]
    fn hiragana_uses_the_ja_override_table() {
        let d = Jadecoder::new();
        // U+3042 HIRAGANA LETTER A.
        assert_eq!(d.decode("\u{3042}"), "a");
    }

    #[test]
    fn kanji_uses_japanese_reading_not_mandarin_pinyin() {
        let d = Jadecoder::new();
        // U+4E00 (ichi/yi, "one"): jacodepoints gives the on'yomi
        // reading "Ichi ", distinct from zhcodepoints' pinyin "Yi " for
        // the identical Han character -- confirms the ja override table
        // is actually in effect, not silently falling back to the
        // Chinese one.
        assert_eq!(d.decode("\u{4e00}"), "Ichi ");
    }

    #[test]
    fn unmapped_codepoint_falls_back_to_question_mark() {
        let d = Jadecoder::new();
        assert_eq!(d.decode("a\u{e000}b"), "a?b");
    }
}
