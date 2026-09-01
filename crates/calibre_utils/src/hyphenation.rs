//! Port of `old_src/src/calibre/utils/hyphenation/hyphenate.py` and
//! `dictionaries.py` (issue #66): soft-hyphen insertion for
//! justified/hyphenated text rendering.
//!
//! # Scope
//!
//! Real: the word- and text-run-level hyphenation logic
//! ([`add_soft_hyphens`]/[`add_soft_hyphens_to_words`]), backed by a
//! genuine Knuth-Liang hyphenation engine (the [`hyphenation`] crate,
//! built from the same TeX/LibreOffice UTF-8 hyphenation patterns
//! upstream's own bundled `dictionaries.tar.xz` is built from) rather
//! than a stub -- this crate embeds the pattern data directly
//! (`embed_all` feature), so there's no missing-asset gap the way
//! there is for e.g. `dynamic.rs`'s `locales.zip` or issue #432's
//! static assets.
//!
//! [`language_for_locale`] replaces `dictionary_name_for_locale` +
//! `locales.json`'s locale-alias table with the `hyphenation` crate's
//! own BCP-47-tag-keyed [`Language`] lookup, plus the same few
//! macro-language defaults (`en` -> American English, etc.) upstream
//! hard-codes for locales with no exact dictionary. This isn't a
//! byte-for-byte port of `locales.json` (not vendored in this
//! project), so a handful of obscure locale aliases upstream's table
//! covers explicitly aren't recognized here -- unrecognized locales
//! simply get no hyphenation (matching upstream's own `None`-dictionary
//! fallback behavior), never a wrong one.
//!
//! Narrowed/not ported: `hyphenate.py`'s HTML-tree walking
//! (`add_soft_hyphens_to_html`/`remove_soft_hyphens_from_html`/
//! `add_to_tag`, including the `tags_not_to_hyphenate` skip-list) is
//! not ported here -- `calibre_utils` has no HTML/DOM tree type to
//! walk (that would be a layering violation; this crate sits below
//! `calibre_ebooks`). Any future in-browser-reader or conversion-
//! pipeline consumer that needs whole-document hyphenation should
//! call [`add_soft_hyphens_to_words`] per text node itself.
//!
//! Simplification enabled by the underlying library: upstream's own
//! `add_soft_hyphens` manually lowercases the word before calling into
//! libhyphen (case-sensitive dictionaries) and re-cases the result
//! afterward. The `hyphenation` crate's own `hyphenate()` is
//! documented as already case-insensitive, so that dance isn't needed
//! here.

use hyphenation::{Hyphenator, Language, Load, Standard};
use regex::Regex;

/// Port of `dictionary_name_for_locale` -- see this module's own doc
/// for how the locale-alias table is narrowed relative to upstream's
/// vendored `locales.json`.
pub fn language_for_locale(locale: &str) -> Option<Language> {
    let normalized = locale.trim().to_lowercase().replace('_', "-");
    if let Some(lang) = Language::try_from_code(&normalized) {
        return Some(lang);
    }
    let primary = normalized.split('-').next().unwrap_or("");
    match primary {
        "en" => Some(Language::EnglishUS),
        "de" => Some(Language::German1996),
        "sr" => Some(Language::SerbianCyrillic),
        "sh" => Some(Language::SerbocroatianLatin),
        _ => Language::try_from_code(primary),
    }
}

/// Loads the embedded Knuth-Liang dictionary for `language`. Port of
/// `dictionary_for_locale` (minus its own locale-to-name lookup, split
/// out as [`language_for_locale`] so callers can cache the loaded
/// [`Standard`] dictionary per [`Language`] themselves).
pub fn dictionary_for_language(language: Language) -> Option<Standard> {
    Standard::from_embedded(language).ok()
}

/// Port of `add_soft_hyphens`: inserts `hyphen_char` (upstream's
/// default is U+00AD SOFT HYPHEN) at every valid hyphenation point in
/// `word`. Returns `word` unchanged if it's too long, contains a
/// literal `=` (both guards inherited from upstream's underlying C
/// hyphenation library, kept here for behavioral parity even though
/// this port's own backing library has neither limitation), or is too
/// short to hyphenate once existing hyphen marks are stripped.
pub fn add_soft_hyphens(word: &str, dictionary: &Standard, hyphen_char: char) -> String {
    if word.chars().count() > 99 || word.contains('=') {
        return word.to_string();
    }
    let bare: String = word.chars().filter(|&c| c != hyphen_char).collect();
    if bare.chars().count() < 4 {
        return word.to_string();
    }

    let hyphenated = dictionary.hyphenate(&bare);
    if hyphenated.breaks.is_empty() {
        return word.to_string();
    }

    let mut result = String::with_capacity(bare.len() + hyphenated.breaks.len());
    let mut last = 0;
    for &at in &hyphenated.breaks {
        result.push_str(&bare[last..at]);
        result.push(hyphen_char);
        last = at;
    }
    result.push_str(&bare[last..]);
    result
}

fn word_pattern() -> &'static Regex {
    use std::sync::OnceLock;
    static PAT: OnceLock<Regex> = OnceLock::new();
    PAT.get_or_init(|| Regex::new(r"\w+").unwrap())
}

/// Port of `add_soft_hyphens_to_words`: hyphenates every word in
/// `text`, leaving whitespace/punctuation between words untouched.
pub fn add_soft_hyphens_to_words(text: &str, dictionary: &Standard, hyphen_char: char) -> String {
    let mut result = String::with_capacity(text.len());
    let mut pos = 0;
    for m in word_pattern().find_iter(text) {
        if m.start() > pos {
            result.push_str(&text[pos..m.start()]);
        }
        result.push_str(&add_soft_hyphens(m.as_str(), dictionary, hyphen_char));
        pos = m.end();
    }
    if pos < text.len() {
        result.push_str(&text[pos..]);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_locale_forms_to_a_dictionary_language() {
        assert_eq!(language_for_locale("en-US"), Some(Language::EnglishUS));
        assert_eq!(language_for_locale("en_US"), Some(Language::EnglishUS));
        assert_eq!(language_for_locale("en"), Some(Language::EnglishUS));
        assert_eq!(language_for_locale("es"), Some(Language::Spanish));
        assert_eq!(language_for_locale("es_MX"), Some(Language::Spanish));
        assert_eq!(language_for_locale("de"), Some(Language::German1996));
        assert_eq!(language_for_locale("xx-not-a-real-locale"), None);
    }

    #[test]
    fn hyphenates_the_classic_knuth_liang_example() {
        let dict = dictionary_for_language(Language::EnglishUS).unwrap();
        assert_eq!(add_soft_hyphens("hyphenation", &dict, '\u{ad}'), "hy\u{ad}phen\u{ad}a\u{ad}tion");
    }

    #[test]
    fn is_case_insensitive() {
        let dict = dictionary_for_language(Language::EnglishUS).unwrap();
        assert_eq!(add_soft_hyphens("CAPITAL", &dict, '\u{ad}'), "CAP\u{ad}I\u{ad}TAL");
    }

    #[test]
    fn leaves_short_words_unchanged() {
        let dict = dictionary_for_language(Language::EnglishUS).unwrap();
        assert_eq!(add_soft_hyphens("cat", &dict, '\u{ad}'), "cat");
    }

    #[test]
    fn hyphenates_every_word_in_a_run_of_text() {
        let dict = dictionary_for_language(Language::EnglishUS).unwrap();
        let out = add_soft_hyphens_to_words("hyphenation, please!", &dict, '\u{ad}');
        assert_eq!(out, "hy\u{ad}phen\u{ad}a\u{ad}tion, please!");
    }

    #[test]
    fn removes_pre_existing_hyphen_marks_before_re_hyphenating() {
        let dict = dictionary_for_language(Language::EnglishUS).unwrap();
        let already = "hy\u{ad}phenation";
        assert_eq!(add_soft_hyphens(already, &dict, '\u{ad}'), "hy\u{ad}phen\u{ad}a\u{ad}tion");
    }
}
