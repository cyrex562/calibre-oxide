//! Port of the language-code half of
//! `old_src/src/calibre/utils/localization.py` (issue #140):
//! `canonicalize_lang`/`lang_as_iso639_1`, mapping any of the ways a
//! book's metadata might spell a language (`English`, `eng`, `en-GB`,
//! `en_US`) onto a canonical code.
//!
//! # Scope
//!
//! Real: [`canonicalize_lang`] and [`lang_as_iso639_1`]'s own logic
//! (lowercase/strip, `_`->`-`, take the primary subtag, try
//! 2-letter/3-letter code lookup, fall back to an English-name
//! lookup), backed by the `isolang` crate's real ISO 639 tables
//! instead of upstream's own bundled `iso639.calibre_msgpack`
//! resource (not vendored in this project -- the same class of gap as
//! `dynamic.rs`'s `locales.zip`, but here a suitable crate exists).
//!
//! Narrowed: upstream's canonical 3-letter code comes from its own
//! `by_3`/`by_3t` table, which for the ~20 languages where ISO 639-2
//! has distinct bibliographic ("ger") and terminology ("deu") codes,
//! prefers the terminology form. `isolang`'s own canonical
//! [`isolang::Language::to_639_3`] output is used as-is here; it
//! matches calibre's choice for the vast majority of languages but
//! isn't guaranteed byte-identical for every B/T-divergent code.
//! `localization.py`'s translation-catalog machinery (`get_lang`,
//! `set_translators`, etc.) is out of scope -- only the language-code
//! half, matching the issue.
//!
//! Everything upstream (`archive.rs`'s `TODO`, `docx/container.rs`,
//! `docx/writer/container.rs`, `utils::open_with::linux`'s
//! `localize_string`) was waiting on this is now a one-line call.

use isolang::Language;

/// Port of `canonicalize_lang`: maps a language name or code (in
/// pretty much any spelling a book might carry) onto its canonical
/// ISO 639-3 code, or `None` if unrecognized.
pub fn canonicalize_lang(raw: &str) -> Option<String> {
    let raw = raw.to_lowercase();
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let raw = raw.replace('_', "-");
    let primary = raw.split('-').next().unwrap_or("").trim();
    if primary.is_empty() {
        return None;
    }

    if primary.len() == 2 {
        if let Some(lang) = Language::from_639_1(primary) {
            return Some(lang.to_639_3().to_string());
        }
    } else if primary.len() == 3 {
        if let Some(lang) = Language::from_639_3(primary) {
            return Some(lang.to_639_3().to_string());
        }
    }

    Language::from_name_lowercase(primary).map(|l| l.to_639_3().to_string())
}

/// Port of `lang_as_iso639_1`: narrows [`canonicalize_lang`]'s result
/// to a two-letter code, or `None` if the language has none (most
/// living languages do; many historical/constructed/macro languages
/// don't).
pub fn lang_as_iso639_1(name_or_code: &str) -> Option<String> {
    let code = canonicalize_lang(name_or_code)?;
    Language::from_639_3(&code).and_then(|l| l.to_639_1()).map(str::to_string)
}

/// Port of `calibre_langcode_to_name`'s English-name half (issue
/// #514's `language_strings()` builtin): the display name for a
/// language code. Narrowed vs. upstream: only the English name is
/// available (`isolang`'s `english_names` feature) -- there's no
/// translation-catalog machinery in this crate to localize into the
/// current locale, so a `localize != 0` request still gets the
/// English name rather than erroring or silently returning the raw
/// code.
pub fn lang_display_name(code: &str) -> Option<String> {
    let canonical = canonicalize_lang(code)?;
    Language::from_639_3(&canonical).map(|l| l.to_name().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_two_letter_codes() {
        assert_eq!(canonicalize_lang("en"), Some("eng".to_string()));
        assert_eq!(canonicalize_lang("EN"), Some("eng".to_string()));
    }

    #[test]
    fn canonicalizes_locale_style_codes() {
        assert_eq!(canonicalize_lang("en-GB"), Some("eng".to_string()));
        assert_eq!(canonicalize_lang("en_US"), Some("eng".to_string()));
    }

    #[test]
    fn lang_display_name_maps_a_code_to_its_english_name() {
        assert_eq!(lang_display_name("en"), Some("English".to_string()));
        assert_eq!(lang_display_name("fra"), Some("French".to_string()));
        // "not" alone is coincidentally a real ISO 639-3 code
        // (Nomatsiguenga) -- use a string with no valid subtag at all.
        assert_eq!(lang_display_name("zzzzzzzz"), None);
    }

    #[test]
    fn canonicalizes_three_letter_codes() {
        assert_eq!(canonicalize_lang("eng"), Some("eng".to_string()));
    }

    #[test]
    fn canonicalizes_english_names() {
        assert_eq!(canonicalize_lang("English"), Some("eng".to_string()));
        assert_eq!(canonicalize_lang("french"), Some("fra".to_string()));
    }

    #[test]
    fn returns_none_for_unrecognized_input() {
        assert_eq!(canonicalize_lang(""), None);
        assert_eq!(canonicalize_lang("zzzzz-not-a-real-language"), None);
    }

    #[test]
    fn narrows_to_iso639_1() {
        assert_eq!(lang_as_iso639_1("English"), Some("en".to_string()));
        assert_eq!(lang_as_iso639_1("eng"), Some("en".to_string()));
    }

    #[test]
    fn iso639_1_is_none_for_languages_with_no_two_letter_code() {
        // Ainu has an ISO 639-3 code (ain) but no 639-1 code.
        assert_eq!(lang_as_iso639_1("ain"), None);
    }
}
