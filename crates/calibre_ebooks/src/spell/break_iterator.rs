//! Port of `calibre.spell.break_iterator`.
//!
//! # Word/sentence segmentation: `unicode-segmentation`, not ICU
//!
//! Upstream wraps ICU's `BreakIterator` (`UBRK_WORD`/`UBRK_SENTENCE`),
//! a locale-aware, dictionary-based segmenter -- important for
//! languages without whitespace between words (Thai, Japanese, ...).
//! This workspace has no ICU binding, so every function here uses the
//! `unicode-segmentation` crate instead: a pure-Rust implementation of
//! Unicode UAX #29 word/sentence-boundary rules, locale-independent
//! (the `lang` parameter is accepted everywhere for API parity but
//! unused). Correct for the overwhelming majority of real-world text
//! (anything using whitespace/punctuation to separate words); will not
//! segment dictionary-requiring scripts as accurately as ICU. Same
//! documented tradeoff as `oeb::polish::spell` (which imports
//! [`split_into_words`]/[`index_of`] from here rather than keeping its
//! own copy).
//!
//! Positions returned here are **byte offsets** into the `&str` passed
//! in, unlike upstream's `split2()` (which upstream itself converts
//! from UTF-16 code units to Unicode code-point counts, "suitable for
//! indexing python strings"). Byte offsets are the correct choice for
//! indexing a Rust `&str`; there is no code-point-counting quirk to
//! preserve here since it existed only to match Python's own string
//! indexing.
//!
//! `split2()`'s hyphen-recombination (ICU splits "well-known" into
//! three tokens; upstream's C extension manually re-merges adjacent
//! hyphen-separated words back into one) is **not** replicated --
//! `unicode_words()`/`unicode_word_indices()` just report each side of
//! a hyphen as its own word. This is a real, narrower fidelity gap than
//! the locale one above (it affects common English text, not just
//! exotic scripts), but implementing upstream's exact merge-chain
//! algorithm on top of a different underlying segmenter risked
//! producing subtly wrong results that would be harder to notice than
//! an honestly-simpler pass-through. Flagged here for anyone who hits
//! it in practice.

use lazy_static::lazy_static;
use regex::Regex;
use unicode_segmentation::UnicodeSegmentation;

/// Port of `split_into_words`.
pub fn split_into_words(text: &str, _lang: &str) -> Vec<String> {
    text.unicode_words().map(|w| w.to_string()).collect()
}

/// Port of `split_into_words_and_positions`: `(byte offset, byte length)`
/// pairs, one per word.
pub fn split_into_words_and_positions(text: &str, _lang: &str) -> Vec<(usize, usize)> {
    text.unicode_word_indices().map(|(i, w)| (i, w.len())).collect()
}

/// Port of `index_of`: the byte offset of the first word-boundary-aligned
/// occurrence of `needle` in `haystack`, or `None` if absent.
pub fn index_of(needle: &str, haystack: &str, _lang: &str) -> Option<usize> {
    haystack.unicode_word_indices().find(|(_, w)| *w == needle).map(|(i, _)| i)
}

/// Port of `count_words`.
pub fn count_words(text: &str, lang: &str) -> usize {
    split_into_words_and_positions(text, lang).len()
}

/// Port of `sentence_positions`: `(byte offset, byte length)` pairs, one
/// per sentence (including trailing whitespace, matching
/// `unicode-segmentation`'s own UAX #29 sentence-boundary convention).
pub fn sentence_positions(text: &str, _lang: &str) -> Vec<(usize, usize)> {
    text.split_sentence_bound_indices().map(|(i, s)| (i, s.len())).collect()
}

/// Port of `split_into_sentences`.
pub fn split_into_sentences(text: &str, lang: &str) -> Vec<String> {
    sentence_positions(text, lang).into_iter().map(|(p, s)| text[p..p + s].to_string()).collect()
}

/// Port of `split_long_sentences`: if `sentence` (whose own text starts
/// at `offset` within whatever larger text it came from) is short
/// enough, yields it unchanged; otherwise greedily regroups its words
/// into chunks of at most `limit` *characters* (matching upstream's
/// `len()`, which counts Unicode code points, not bytes) each,
/// space-joined. Materialized into a `Vec` rather than upstream's
/// generator, matching this port's "pure function" convention.
pub fn split_long_sentences(sentence: &str, offset: usize, lang: &str, limit: usize) -> Vec<(usize, String)> {
    if sentence.chars().count() <= limit {
        return vec![(offset, sentence.to_string())];
    }
    let mut out = Vec::new();
    let mut buf: Vec<&str> = Vec::new();
    let mut total = 0usize;
    let mut start_at = 0usize;

    for (start, length) in split_into_words_and_positions(sentence, lang) {
        let t = &sentence[start..start + length];
        if buf.is_empty() {
            start_at = start;
        }
        buf.push(t);
        total += t.chars().count();
        if total >= limit {
            out.push((offset + start_at, buf.join(" ")));
            buf.clear();
            total = 0;
        }
    }
    if !buf.is_empty() {
        out.push((offset + start_at, buf.join(" ")));
    }
    out
}

/// Upstream's `PARAGRAPH_SEPARATOR` sentinel, used by
/// [`split_into_sentences_for_tts_embed`]/[`split_into_sentences_for_tts`]
/// to mark a paragraph break without disturbing sentence segmentation.
pub const PARAGRAPH_SEPARATOR: char = '\u{2029}';

lazy_static! {
    static ref MULTI_NEWLINE: Regex = Regex::new(r"\n{2,}").unwrap();
}

/// Replaces `\r` with a space and every run of 2+ `\n` with
/// [`PARAGRAPH_SEPARATOR`] followed by enough spaces to keep the same
/// *character* count (matching upstream's `len(m.group())`-based
/// padding -- a byte-length-preserving guarantee in Python's
/// code-point-indexed strings, though not in this port's byte-indexed
/// ones; see this module's doc on why that distinction doesn't matter
/// here), then replaces any remaining single `\n` with a space.
fn normalize_paragraphs_for_tts(text: &str) -> String {
    let replaced_cr = text.replace('\r', " ");
    let substituted = MULTI_NEWLINE.replace_all(&replaced_cr, |caps: &regex::Captures| {
        let m = &caps[0];
        let mut s = String::new();
        s.push(PARAGRAPH_SEPARATOR);
        for _ in 0..(m.chars().count() - 1) {
            s.push(' ');
        }
        s
    });
    substituted.replace('\n', " ")
}

/// Port of `split_into_sentences_for_tts_embed`. Upstream returns only
/// position/length pairs into its own locally-transformed `text`,
/// implicitly relying on the caller having reproduced the identical
/// `\n`-normalization independently. This port returns the transformed
/// text alongside its positions instead, so the result is
/// self-contained and directly indexable -- a deliberate, disclosed
/// adaptation for a cleaner Rust API, not a behavior change in the
/// segmentation itself.
pub fn split_into_sentences_for_tts_embed(text: &str, lang: &str) -> (String, Vec<(usize, usize)>) {
    let normalized = normalize_paragraphs_for_tts(text);
    let positions = sentence_positions(&normalized, lang);
    (normalized, positions)
}

/// Port of `split_into_sentences_for_tts`: like
/// [`split_into_sentences_for_tts_embed`], but rather than raw sentence
/// boundaries, merges together consecutive short sentences (shorter
/// than `min_sentence_length` characters, unless the sentence ends on a
/// paragraph break) and splits overlong ones via
/// [`split_long_sentences`] (`limit = max_sentence_length`) -- upstream's
/// own algorithm for chunking prose into TTS-appropriately-sized
/// utterances. Positions are into the internally-normalized text (see
/// [`split_into_sentences_for_tts_embed`]'s doc for why that's fine:
/// each returned pair carries its own sentence text, not just an
/// offset needing external re-slicing).
pub fn split_into_sentences_for_tts(text: &str, lang: &str, min_sentence_length: usize, max_sentence_length: usize) -> Vec<(usize, String)> {
    let text = normalize_paragraphs_for_tts(text);

    let mut out = Vec::new();
    let mut pending_start = 0usize;
    let mut pending_sentence = String::new();

    for (start, length) in sentence_positions(&text, lang) {
        let end = start + length;
        let raw = &text[start..end];
        let sentence = raw.trim_end().replace('\n', " ");
        let sentence = sentence.trim().to_string();
        if sentence.is_empty() {
            continue;
        }

        let last_char_before_end = text[..end].chars().next_back();
        if sentence.chars().count() < min_sentence_length && last_char_before_end != Some(PARAGRAPH_SEPARATOR) {
            if !pending_sentence.is_empty() {
                pending_sentence.push(' ');
                pending_sentence.push_str(&sentence);
                if pending_sentence.chars().count() >= min_sentence_length {
                    out.push((pending_start, std::mem::take(&mut pending_sentence)));
                    pending_start = 0;
                }
            } else {
                pending_start = start;
                pending_sentence = sentence;
            }
            continue;
        }

        for (mut s_start, mut s_sentence) in split_long_sentences(&sentence, start, lang, max_sentence_length) {
            if !pending_sentence.is_empty() {
                s_sentence = format!("{pending_sentence} {s_sentence}");
                s_start = pending_start;
                pending_start = 0;
                pending_sentence.clear();
            }
            out.push((s_start, s_sentence));
        }
    }
    if !pending_sentence.is_empty() {
        out.push((pending_start, pending_sentence));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_into_words_extracts_plain_words() {
        let words = split_into_words("Hello, world! It's fine.", "eng");
        assert_eq!(words, vec!["Hello", "world", "It's", "fine"]);
    }

    #[test]
    fn split_into_words_and_positions_round_trips() {
        let text = "one two three";
        for (pos, len) in split_into_words_and_positions(text, "eng") {
            assert!(!text[pos..pos + len].is_empty());
        }
        assert_eq!(split_into_words_and_positions(text, "eng").len(), 3);
    }

    #[test]
    fn index_of_finds_a_whole_word_not_a_substring() {
        assert_eq!(index_of("i", "string i", "eng"), Some(7));
        assert_eq!(index_of("str", "string i", "eng"), None);
    }

    #[test]
    fn count_words_counts_tokens_not_characters() {
        assert_eq!(count_words("one two three", "eng"), 3);
    }

    #[test]
    fn split_into_sentences_splits_on_terminal_punctuation() {
        let sentences = split_into_sentences("One. Two! Three?", "eng");
        assert_eq!(sentences.len(), 3);
        assert!(sentences[0].starts_with("One."));
    }

    #[test]
    fn split_long_sentences_passes_short_sentences_through_unchanged() {
        let out = split_long_sentences("short sentence", 10, "eng", 2048);
        assert_eq!(out, vec![(10, "short sentence".to_string())]);
    }

    #[test]
    fn split_long_sentences_chunks_by_character_limit() {
        let sentence = "one two three four five six seven eight nine ten";
        let out = split_long_sentences(sentence, 0, "eng", 12);
        assert!(out.len() > 1);
        for (_, chunk) in &out {
            assert!(chunk.chars().count() <= 20, "chunk too long: {chunk:?}");
        }
        let rejoined: String = out.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>().join(" ");
        assert_eq!(rejoined.split_whitespace().collect::<Vec<_>>(), sentence.split_whitespace().collect::<Vec<_>>());
    }

    #[test]
    fn split_into_sentences_for_tts_embed_preserves_paragraph_breaks() {
        let (normalized, positions) = split_into_sentences_for_tts_embed("Para one.\n\nPara two.", "eng");
        assert!(normalized.contains(PARAGRAPH_SEPARATOR));
        assert!(!positions.is_empty());
    }

    #[test]
    fn split_into_sentences_for_tts_merges_short_sentences() {
        let out = split_into_sentences_for_tts("Hi. Ok. This is a longer sentence that stands alone.", "eng", 20, 1024);
        // The two short greetings should be merged into one chunk rather
        // than emitted as separate tiny utterances.
        assert!(out.iter().any(|(_, s)| s.contains("Hi.") && s.contains("Ok.")));
    }

    #[test]
    fn split_into_sentences_for_tts_splits_overlong_sentences() {
        let long_sentence = std::iter::repeat("word ").take(500).collect::<String>() + ".";
        let out = split_into_sentences_for_tts(&long_sentence, "eng", 0, 100);
        assert!(out.len() > 1, "expected the overlong sentence to be chunked");
        for (_, chunk) in &out {
            assert!(chunk.chars().count() <= 200, "chunk unexpectedly long: {} chars", chunk.chars().count());
        }
    }
}
