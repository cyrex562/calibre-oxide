//! Port of `old_src/src/calibre/ebooks/unihandecode/unidecoder.py`'s
//! `Unidecoder` -- and the shared table-lookup machinery
//! `Krdecoder`/`Vndecoder`/`Jadecoder`'s codepoint-fallback path all
//! reuse, since in the Python those three classes only ever override
//! `Unidecoder.__init__`'s `codepoints` dict, not `decode`/
//! `replace_point`/`code_group`/`grouped_point`.
//!
//! # The algorithm
//!
//! For each non-ASCII character, split its codepoint into a block
//! (`codepoint >> 8`) and an index within that block
//! (`codepoint & 0xff`), and look up `table[block][index]`. Python
//! wraps that double lookup in `try/except Exception: return '?'`; the
//! two ways it can fail -- no such block, or a block that exists but
//! whose list is shorter than `index + 1` (the last block in a file is
//! sometimes truncated below 256 entries) -- both collapse to the same
//! `"?"` fallback here. ASCII characters are never looked up at all,
//! matching Python's `re.sub(r'[^\x00-\x7f]', ...)` gate.
//!
//! # Table merging
//!
//! `Krdecoder`/`Vndecoder`/`Unidecoder` each start from
//! `unicodepoints::CODEPOINTS` and layer a language-specific override
//! table on top via `self.codepoints = CODEPOINTS; self.codepoints
//! .update(HANCODES)`. `dict.update` replaces a block's *entire*
//! replacement list when the override defines that block at all --
//! there is no per-codepoint merging within a block -- so [`merge_tables`]
//! does the same: an override block fully replaces the base block, and
//! blocks the override doesn't mention keep the base's list untouched.
//! (The one thing not replicated: Python's `self.codepoints = CODEPOINTS`
//! aliases and then *mutates* the shared module-level dict on every
//! single `Unidecoder()` construction, rather than copying it. Since
//! every construction merges in the exact same override values, that's
//! an idempotent no-op after the first call, not an observable
//! behavior -- it's an accident of the Python, not ported.)

use std::collections::HashMap;

use crate::unihandecode::{unicodepoints, zhcodepoints};

/// A flattened `block -> replacement list` table. Mirrors Python's
/// `dict[str, list[str]]` `CODEPOINTS`, keyed by the integer block id
/// (`codepoint >> 8`) instead of the `"xAB"` string Python only uses to
/// keep the dict literal readable in source.
pub(crate) type BlockMap = HashMap<u32, &'static [&'static str]>;

/// Port of `self.codepoints = BASE; self.codepoints.update(OVERRIDE)`.
/// See the module docs for why this is a whole-block replace, not a
/// per-codepoint merge.
pub(crate) fn merge_tables(
    base: &[(u32, &'static [&'static str])],
    overrides: &[(u32, &'static [&'static str])],
) -> BlockMap {
    let mut map: BlockMap = base.iter().copied().collect();
    for &(block, entries) in overrides {
        map.insert(block, entries);
    }
    map
}

/// Port of `replace_point` + `code_group` + `grouped_point` combined:
/// looks up one non-ASCII character's replacement, or `"?"` if its
/// block/index isn't covered.
pub(crate) fn replace_point(map: &BlockMap, ch: char) -> &'static str {
    let cp = ch as u32;
    let block = cp >> 8;
    let index = (cp & 0xff) as usize;
    map.get(&block)
        .and_then(|entries| entries.get(index))
        .copied()
        .unwrap_or("?")
}

/// Port of `decode`: every non-ASCII character is replaced via
/// [`replace_point`]; ASCII passes through unchanged.
pub(crate) fn decode_with_table(map: &BlockMap, text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else {
            out.push_str(replace_point(map, ch));
        }
    }
    out
}

/// Port of `Unidecoder`: the default decoder ("zh and others" in
/// `Unihandecoder.__init__`), merging the Chinese overrides into the
/// base Unicode table.
pub struct Unidecoder {
    map: BlockMap,
}

impl Unidecoder {
    pub fn new() -> Self {
        Self {
            map: merge_tables(unicodepoints::CODEPOINTS, zhcodepoints::CODEPOINTS),
        }
    }

    /// Replace every non-ASCII character in `text` with its ASCII
    /// transliteration (or `"?"` if none is known).
    pub fn decode(&self, text: &str) -> String {
        decode_with_table(&self.map, text)
    }
}

impl Default for Unidecoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_passes_through_unchanged() {
        let d = Unidecoder::new();
        assert_eq!(d.decode("Hello, World! 123"), "Hello, World! 123");
    }

    #[test]
    fn accented_latin_transliterates() {
        let d = Unidecoder::new();
        // e-acute -> 'e' per unicodepoints' x00e5 block (matches Text::Unidecode).
        assert_eq!(d.decode("caf\u{e9}"), "cafe");
    }

    #[test]
    fn unmapped_codepoint_falls_back_to_question_mark() {
        let d = Unidecoder::new();
        // U+E000: start of the Private Use Area -- no block covers it.
        assert_eq!(d.decode("a\u{e000}b"), "a?b");
    }

    #[test]
    fn chinese_characters_use_the_zh_override_table() {
        let d = Unidecoder::new();
        // U+4E00 (yi1, "one") -- zhcodepoints overrides unicodepoints'
        // x4e block, which otherwise has no entry for CJK ideographs.
        assert_eq!(d.decode("\u{4e00}"), "Yi ");
    }

    #[test]
    fn empty_table_entry_deletes_the_character_rather_than_using_question_mark() {
        let d = Unidecoder::new();
        // U+0080 (a C1 control character) has a real, defined-but-empty
        // ('') entry in unicodepoints' x00 block -- confirm that
        // deletes the character rather than falling back to '?', which
        // only happens for codepoints with no entry at all.
        let out = d.decode("a\u{80}b");
        assert_eq!(out, "ab", "{out}");
    }
}
