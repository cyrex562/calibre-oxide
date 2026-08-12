//! Text tokenization + stemming for full-text search.
//!
//! Port of `old_src/src/calibre/db/sqlite_extension.cpp`. The C++
//! original does three jobs:
//!
//! 1. Word-level tokenization of arbitrary UTF-8, respecting Unicode
//!    boundaries and handling script transitions (Han/Thai/Khmer/...
//!    switch to per-script BreakIterators).
//! 2. Snowball stemming per language.
//! 3. Registration of the tokenizers as FTS5 custom tokenizers on a
//!    SQLite connection (`calibre_sqlite_extension_init`).
//!
//! Rust port coverage:
//! - **(1) tokenization**: `unicode-segmentation` — good for
//!   Latin/Cyrillic/Greek/etc. It uses UAX #29 word boundaries.
//!   Dictionary-based segmentation for CJK/Thai/etc. is inferior to
//!   ICU's; see follow-up note.
//! - **(2) stemming**: `rust-stemmers` — pure-Rust Snowball ports,
//!   covers the same languages as libstemmer.
//! - **(3) diacritic removal**: `unicode-normalization` NFD + strip
//!   combining marks.
//! - **FTS5 tokenizer registration**: NOT included here — it's a
//!   rusqlite-integration concern that belongs alongside connection
//!   opening in the fault-tolerance-aware `LibraryHandle` port
//!   (issue #93). Registered as a placeholder below.

use std::fmt;

use rust_stemmers::{Algorithm, Stemmer as SbStemmer};
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};
use unicode_segmentation::UnicodeSegmentation;

/// A single token emitted by [`Tokenizer::tokenize`].
///
/// `text` is case-folded (lowercased) and, when requested, has
/// combining diacritics stripped. `start` / `end` are byte offsets
/// into the ORIGINAL input string — critical for SQLite FTS5's
/// snippet/highlight machinery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub text: String,
    pub start: usize,
    pub end: usize,
}

/// The tokenization mode. FTS5 passes different flags at index time
/// vs query time; the C++ Tokenizer used the flag to decide whether
/// to emit a diacritic-stripped duplicate ("colocated" token) for
/// index-time only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenizeMode {
    /// Index-time. If `remove_diacritics` is set, also emit a
    /// diacritic-stripped duplicate token at the same offsets.
    Document,
    /// Query-time. Only emit the primary token — do not duplicate
    /// (the C++ version skipped the diacritic-strip branch on
    /// queries).
    Query,
}

pub struct Tokenizer {
    /// If true, at Document-time we emit an extra diacritic-stripped
    /// token per input word. See `TokenizeMode` docs.
    pub remove_diacritics: bool,
    /// If Some, apply Snowball stemming for the given language.
    /// The C++ registered the "porter" tokenizer specifically as the
    /// stemming one; the plain "calibre" tokenizer had no stemmer.
    pub stemmer: Option<Algorithm>,
}

impl Tokenizer {
    /// The default `calibre` tokenizer: no stemmer, diacritics
    /// stripped at index time.
    pub fn calibre() -> Self {
        Self {
            remove_diacritics: true,
            stemmer: None,
        }
    }

    /// The `porter` tokenizer: with English Snowball stemming and
    /// diacritics stripped.
    pub fn porter_english() -> Self {
        Self {
            remove_diacritics: true,
            stemmer: Some(Algorithm::English),
        }
    }

    pub fn tokenize(&self, text: &str, mode: TokenizeMode) -> Vec<Token> {
        let mut out = Vec::new();
        let stemmer = self.stemmer.map(SbStemmer::create);

        for (start, word) in text.split_word_bound_indices() {
            if !is_word(word) {
                continue;
            }
            let end = start + word.len();
            let folded = case_fold(word);
            let stemmed = match &stemmer {
                Some(s) => s.stem(&folded).into_owned(),
                None => folded.clone(),
            };
            out.push(Token {
                text: stemmed,
                start,
                end,
            });

            if matches!(mode, TokenizeMode::Document) && self.remove_diacritics {
                let stripped = strip_diacritics(&folded);
                if stripped != folded {
                    // Emit the diacritic-stripped token as a colocated
                    // sibling — same byte offsets, so FTS5 treats it as
                    // an alternate for the same source position.
                    let stripped_stemmed = match &stemmer {
                        Some(s) => s.stem(&stripped).into_owned(),
                        None => stripped,
                    };
                    out.push(Token {
                        text: stripped_stemmed,
                        start,
                        end,
                    });
                }
            }
        }
        out
    }
}

/// Snowball language keys the C++ libstemmer supported. The Rust
/// `rust_stemmers::Algorithm` enum covers the same set except for a
/// handful of less-common ones (Basque, Catalan, ...) — those either
/// aren't in the pure-Rust port or use a different key. Keep the list
/// stable so the FTS layer can enumerate available stemmers.
pub const AVAILABLE_STEMMER_LANGUAGES: &[&str] = &[
    "arabic",
    "danish",
    "dutch",
    "english",
    "finnish",
    "french",
    "german",
    "greek",
    "hungarian",
    "italian",
    "norwegian",
    "portuguese",
    "romanian",
    "russian",
    "spanish",
    "swedish",
    "tamil",
    "turkish",
];

pub fn stemmer_for_language(lang: &str) -> Option<Algorithm> {
    match lang.to_ascii_lowercase().as_str() {
        "arabic" => Some(Algorithm::Arabic),
        "danish" => Some(Algorithm::Danish),
        "dutch" => Some(Algorithm::Dutch),
        "english" | "en" => Some(Algorithm::English),
        "finnish" => Some(Algorithm::Finnish),
        "french" | "fr" => Some(Algorithm::French),
        "german" | "de" => Some(Algorithm::German),
        "greek" => Some(Algorithm::Greek),
        "hungarian" => Some(Algorithm::Hungarian),
        "italian" => Some(Algorithm::Italian),
        "norwegian" => Some(Algorithm::Norwegian),
        "portuguese" => Some(Algorithm::Portuguese),
        "romanian" => Some(Algorithm::Romanian),
        "russian" => Some(Algorithm::Russian),
        "spanish" | "es" => Some(Algorithm::Spanish),
        "swedish" => Some(Algorithm::Swedish),
        "tamil" => Some(Algorithm::Tamil),
        "turkish" => Some(Algorithm::Turkish),
        _ => None,
    }
}

/// Apply Snowball stemming to `word` for the given language. Returns
/// the input unchanged if no stemmer is available for that language.
pub fn stem(word: &str, lang: &str) -> String {
    match stemmer_for_language(lang) {
        Some(algo) => SbStemmer::create(algo).stem(word).into_owned(),
        None => word.to_string(),
    }
}

/// Predicate mirroring the C++ `is_token_char` — a token contains at
/// least one letter, digit, or currency/other-symbol.
fn is_word(s: &str) -> bool {
    s.chars().any(|c| {
        c.is_alphanumeric()
            || matches!(c, '$' | '€' | '£' | '¥' | '¢' | '₹' | '₽' | '₩')
    })
}

/// Case-fold to lowercase. Rust `to_lowercase` is Unicode-correct
/// (folds ß → ss, dotted-I → i in Turkish etc. via the default
/// mapping — good enough for FTS).
fn case_fold(s: &str) -> String {
    s.to_lowercase()
}

/// NFD decomposition, strip combining marks, recompose. This is the
/// standard "remove diacritics" pattern.
fn strip_diacritics(s: &str) -> String {
    s.nfd().filter(|c| !is_combining_mark(*c)).collect()
}

/// Sentinel error type for the FTS5 registration placeholder.
#[derive(Debug)]
pub struct Fts5NotWired;
impl fmt::Display for Fts5NotWired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FTS5 custom tokenizer registration not yet wired — tracked as issue #93 (LibraryHandle)")
    }
}
impl std::error::Error for Fts5NotWired {}

/// Register the `calibre` and `porter` FTS5 tokenizers on a rusqlite
/// connection. Deliberately unimplemented in this port — the C++
/// version registers via `fts5_api->xCreateTokenizer`, which rusqlite
/// doesn't expose directly. Wiring it correctly requires either:
/// (a) the `rusqlite_ext_fts5` crate, or (b) hand-rolled FFI against
/// the SQLite extension API.
///
/// This lands as part of the `LibraryHandle` port (issue #93) so the
/// registration happens exactly once per Connection at open time,
/// alongside the WAL/synchronous=FULL pragmas.
pub fn register_fts5_tokenizers(_conn: &rusqlite::Connection) -> Result<(), Fts5NotWired> {
    // todo!("placeholder: wire rusqlite fts5 tokenizer registration in #93")
    Err(Fts5NotWired)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_simple_english() {
        let t = Tokenizer::calibre();
        let toks = t.tokenize("Hello, world!", TokenizeMode::Query);
        let words: Vec<&str> = toks.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(words, vec!["hello", "world"]);
    }

    #[test]
    fn tokenize_preserves_byte_offsets() {
        let t = Tokenizer::calibre();
        let text = "Hello, world!";
        let toks = t.tokenize(text, TokenizeMode::Query);
        assert_eq!(toks[0].start, 0);
        assert_eq!(toks[0].end, 5);
        assert_eq!(&text[toks[0].start..toks[0].end], "Hello");
        // "world" starts after ", "
        assert_eq!(toks[1].start, 7);
        assert_eq!(toks[1].end, 12);
        assert_eq!(&text[toks[1].start..toks[1].end], "world");
    }

    #[test]
    fn tokenize_skips_pure_punctuation() {
        let t = Tokenizer::calibre();
        let toks = t.tokenize("... !!! -- ", TokenizeMode::Query);
        assert!(toks.is_empty(), "got: {:?}", toks);
    }

    #[test]
    fn tokenize_lowercases() {
        let t = Tokenizer::calibre();
        let toks = t.tokenize("FOO BaR", TokenizeMode::Query);
        assert_eq!(toks[0].text, "foo");
        assert_eq!(toks[1].text, "bar");
    }

    #[test]
    fn tokenize_query_does_not_duplicate_for_diacritics() {
        // Even with remove_diacritics=true (the calibre default),
        // query mode must emit exactly one token per word — the
        // primary. Only Document mode emits colocated stripped
        // duplicates.
        let t = Tokenizer::calibre();
        let toks = t.tokenize("café", TokenizeMode::Query);
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0].text, "café");
    }

    #[test]
    fn tokenize_document_emits_colocated_diacritic_stripped_duplicate() {
        let t = Tokenizer::calibre();
        let toks = t.tokenize("café", TokenizeMode::Document);
        assert_eq!(toks.len(), 2);
        assert_eq!(toks[0].text, "café");
        assert_eq!(toks[1].text, "cafe");
        // Colocated: same byte offsets.
        assert_eq!(toks[0].start, toks[1].start);
        assert_eq!(toks[0].end, toks[1].end);
    }

    #[test]
    fn tokenize_document_no_duplicate_if_no_diacritics() {
        let t = Tokenizer::calibre();
        let toks = t.tokenize("hello", TokenizeMode::Document);
        assert_eq!(toks.len(), 1);
    }

    #[test]
    fn tokenize_document_no_duplicate_when_remove_diacritics_off() {
        let t = Tokenizer {
            remove_diacritics: false,
            stemmer: None,
        };
        let toks = t.tokenize("café", TokenizeMode::Document);
        assert_eq!(toks.len(), 1);
    }

    #[test]
    fn tokenize_with_porter_stemmer_stems() {
        let t = Tokenizer::porter_english();
        let toks = t.tokenize("running runners", TokenizeMode::Query);
        // Snowball English stems both to "run".
        assert_eq!(toks[0].text, "run");
        assert_eq!(toks[1].text, "runner");
    }

    #[test]
    fn tokenize_handles_currency_and_digits() {
        let t = Tokenizer::calibre();
        let toks = t.tokenize("$5 €10 3.14", TokenizeMode::Query);
        let words: Vec<&str> = toks.iter().map(|t| t.text.as_str()).collect();
        // Digits are alphanumeric so they tokenize; currency is caught
        // by the is_word predicate.
        assert!(words.contains(&"5"), "got: {:?}", words);
        assert!(words.contains(&"10"), "got: {:?}", words);
    }

    #[test]
    fn stem_falls_back_to_input_on_unknown_language() {
        assert_eq!(stem("running", "klingon"), "running");
    }

    #[test]
    fn stem_english_reduces_inflections() {
        assert_eq!(stem("running", "english"), "run");
        assert_eq!(stem("fishing", "en"), "fish");
    }

    #[test]
    fn stemmer_for_language_accepts_common_aliases() {
        assert!(stemmer_for_language("en").is_some());
        assert!(stemmer_for_language("EN").is_some());
        assert!(stemmer_for_language("English").is_some());
        assert!(stemmer_for_language("ENGLISH").is_some());
        assert!(stemmer_for_language("Klingon").is_none());
    }

    #[test]
    fn strip_diacritics_matches_common_cases() {
        assert_eq!(strip_diacritics("café"), "cafe");
        assert_eq!(strip_diacritics("naïve"), "naive");
        assert_eq!(strip_diacritics("Ångström"), "Angstrom");
        // No-op when no combining marks present.
        assert_eq!(strip_diacritics("hello"), "hello");
    }

    #[test]
    fn register_fts5_returns_placeholder_error() {
        // Documents the placeholder — when #93 lands, this test
        // becomes the real registration test.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let err = register_fts5_tokenizers(&conn).unwrap_err();
        // Just verify the message references the tracking issue.
        assert!(format!("{}", err).contains("#93"));
    }
}
