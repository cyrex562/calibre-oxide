//! Word hyphenation.
//!
//! Port of `old_src/src/calibre/ebooks/hyphenate.py`. The Python
//! original bundled a hard-coded English hyphenation dictionary (526
//! lines including the dict). Rust delegates to the `hyphenation`
//! crate, which uses the same Knuth-Liang TeX-style algorithm and
//! ships language dictionaries as separate feature-gated data files.
//!
//! Only English (en-US) is embedded by default (via the
//! `embed_en-us` feature). If additional languages are needed later,
//! either enable more `embed_*` features in `Cargo.toml` or load a
//! dictionary from disk via the `hyphenation::Load` trait.

use std::sync::OnceLock;

use hyphenation::{Hyphenator, Language, Load, Standard};

/// Return the language pattern set for `en-US`, initializing on
/// first use. The load is expensive-ish (parses an embedded binary
/// pattern table) so we cache it.
fn en_us_hyphenator() -> &'static Standard {
    static H: OnceLock<Standard> = OnceLock::new();
    H.get_or_init(|| {
        // The embed_en-us feature guarantees this loads without I/O.
        Standard::from_embedded(Language::EnglishUS)
            .expect("en-US hyphenation patterns embedded at compile time")
    })
}

/// Hyphenate a single word for English. Returns a `Vec<String>` of
/// syllable parts (matching the Python `hyphenate_word(word) ->
/// list[str]` shape). Non-words (containing digits, punctuation) are
/// returned as a single-element vec containing the original word,
/// matching the Python fallback behavior.
pub fn hyphenate_word(word: &str) -> Vec<String> {
    if word.is_empty() {
        return vec![String::new()];
    }
    if !word.chars().all(|c| c.is_alphabetic()) {
        return vec![word.to_string()];
    }
    let hyphenated = en_us_hyphenator().hyphenate(word);
    let mut out = Vec::with_capacity(hyphenated.breaks.len() + 1);
    let mut prev = 0usize;
    for &pos in &hyphenated.breaks {
        out.push(word[prev..pos].to_string());
        prev = pos;
    }
    out.push(word[prev..].to_string());
    out
}

/// Hyphenate all words in a run of text, preserving whitespace and
/// punctuation between words. Words are joined with the given `sep`
/// (typically `"-"` for hard hyphens or `"\u{00AD}"` for soft hyphens
/// per HTML best practice).
pub fn hyphenate_text(text: &str, sep: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    for c in text.chars() {
        if c.is_alphabetic() {
            word.push(c);
        } else {
            if !word.is_empty() {
                let parts = hyphenate_word(&word);
                out.push_str(&parts.join(sep));
                word.clear();
            }
            out.push(c);
        }
    }
    if !word.is_empty() {
        let parts = hyphenate_word(&word);
        out.push_str(&parts.join(sep));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyphenate_empty_returns_single_empty_part() {
        assert_eq!(hyphenate_word(""), vec![""]);
    }

    #[test]
    fn hyphenate_common_words() {
        // "hyphenation" → "hy·phen·a·tion" (or similar) via TeX
        // patterns. The exact break count depends on the dictionary,
        // so we assert on the round-trip rather than a specific count.
        let parts = hyphenate_word("hyphenation");
        assert!(parts.len() > 1, "expected multiple syllables: {parts:?}");
        assert_eq!(parts.concat(), "hyphenation");
    }

    #[test]
    fn hyphenate_short_word_returns_single_part() {
        // Words too short for Knuth-Liang to break stay as one part.
        assert_eq!(hyphenate_word("a"), vec!["a"]);
        assert_eq!(hyphenate_word("is"), vec!["is"]);
    }

    #[test]
    fn hyphenate_non_alphabetic_returns_original_unchanged() {
        assert_eq!(hyphenate_word("abc123"), vec!["abc123"]);
        assert_eq!(hyphenate_word("hello!"), vec!["hello!"]);
    }

    #[test]
    fn hyphenate_text_preserves_whitespace_and_punctuation() {
        let out = hyphenate_text("The quick, hyphenation test.", "-");
        // Whitespace and punctuation must survive verbatim.
        assert!(out.contains(' '));
        assert!(out.contains(','));
        assert!(out.contains('.'));
        // Round-trip: removing the inserted separator gives us the
        // original text back.
        let restored = out.replace('-', "");
        assert_eq!(restored, "The quick, hyphenation test.");
    }

    #[test]
    fn hyphenate_text_empty_is_empty() {
        assert_eq!(hyphenate_text("", "-"), "");
    }

    #[test]
    fn hyphenate_text_uses_soft_hyphen_when_asked() {
        // Passing U+00AD (soft hyphen) is the canonical HTML-friendly
        // way to mark breakpoints without visible dashes.
        let out = hyphenate_text("hyphenation", "\u{00AD}");
        assert!(out.contains('\u{00AD}'));
        assert_eq!(out.replace('\u{00AD}', ""), "hyphenation");
    }
}
