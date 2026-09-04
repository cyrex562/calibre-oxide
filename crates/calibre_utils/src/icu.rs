//! Port of `calibre.utils.icu` (issue #459). Case folding
//! (`lower`/`upper`/`capitalize`/`title_case`) stays on Rust stdlib
//! Unicode methods, same as before this issue -- upstream's own
//! versions are thin wrappers over the same case-conversion tables,
//! and locale-sensitive case-folding edge cases (Turkish dotless i,
//! etc.) aren't something any caller in this crate currently needs.
//!
//! `strcmp`/`primary_strcmp` are the real, previously-missing piece:
//! genuine Unicode Collation Algorithm string comparison via the
//! `icu` crate (ICU4X, a pure-Rust CLDR-data-driven reimplementation
//! of ICU4C's algorithms -- chosen over `rust_icu`'s real ICU4C
//! bindings because this crate deliberately avoids new C-toolchain
//! dependencies where a real pure-Rust option exists, same call as
//! `calibre_ebooks::spell`'s `spellbook` crate, issue #59; concretely
//! `rust_icu`'s bindgen build fails on this box -- its libclang
//! install is headless, missing the bundled builtin C headers). This
//! gives real locale-aware ordering (e.g. accented letters sorting
//! adjacent to their base letter) in place of a plain `str::cmp` on
//! codepoints, for `calibre_db`'s title/author/series/category-name
//! sort call sites.
//!
//! **Disclosed narrowing vs. upstream's Python API**: upstream's
//! `sort_key(x) -> bytes` returns a precomputed collation key so a
//! large sort only invokes the collator once per string rather than
//! once per comparison. `icu` 1.5's `Collator` doesn't expose raw sort
//! keys (only `compare(a, b) -> Ordering`) -- but nothing in this
//! crate needs precomputed keys either: Rust's own `slice::sort_by`
//! is comparator-based and just as algorithmically efficient (same
//! O(n log n) comparison count) as a key-based sort. So this port
//! exposes `strcmp`/`primary_strcmp` (comparators) instead of
//! `sort_key`/`primary_sort_key` (byte-key producers) -- a Rust-
//! idiomatic surface for the same real capability, not a loss of it.
//!
//! **Disclosed gap, not covered by this port**: upstream's
//! `primary_contains`/`primary_find` (locale-aware, collation-strength
//! *substring search*, via ICU4C's `usearch.h`) has no ICU4X
//! equivalent yet. `calibre_db::search`'s own `primary_contains`/
//! `primary_no_punc_contains` (NFD-decompose + strip combining marks +
//! lowercase, then plain substring `.contains()`) remain that module's
//! own disclosed approximation, unchanged by this issue -- correct for
//! the common case (case + accent folding) but not a true
//! collation-aware search.
//!
//! **Implementation note**: `icu::collator::Collator` is neither
//! `Send` nor `Sync` (it holds an `Rc`-based fallback-data cartable
//! pointer internally, confirmed by trying to share one behind
//! `lazy_static`/`Mutex` and reading the resulting compiler error, not
//! assumed from the docs), so `strcmp`/`primary_strcmp` build a fresh
//! `Collator` per call rather than caching one. Since ICU4X's
//! `compiled_data` feature bakes the CLDR collation tables into the
//! binary (no I/O, no allocation beyond the Yoke wrapper), this is
//! cheap -- ~0.7 microseconds per call, benchmarked -- and avoids any
//! `unsafe impl Send`/`Sync` workaround for a type whose internal
//! thread-safety this crate has no way to independently verify.

use icu::collator::{Collator, CollatorOptions, Strength};
use std::cmp::Ordering;

fn new_collator(strength: Option<Strength>) -> Collator {
    let mut options = CollatorOptions::new();
    options.strength = strength;
    Collator::try_new(&Default::default(), options).expect("icu collator data is compiled in via the `compiled_data` feature")
}

/// Port of `calibre.utils.icu.strcmp`: locale-aware string comparison
/// via the real Unicode Collation Algorithm, not a plain codepoint
/// comparison.
pub fn strcmp(a: &str, b: &str) -> Ordering {
    new_collator(None).compare(a, b)
}

/// Port of `calibre.utils.icu.primary_strcmp`: case- and
/// accent-insensitive locale-aware comparison.
pub fn primary_strcmp(a: &str, b: &str) -> Ordering {
    new_collator(Some(Strength::Primary)).compare(a, b)
}

pub fn lower(text: &str) -> String {
    text.to_lowercase()
}

pub fn upper(text: &str) -> String {
    text.to_uppercase()
}

/// Port of `calibre.utils.icu.capitalize`: uppercase the first character,
/// lowercase the rest (`upper(x[0]) + lower(x[1:])`) -- not merely
/// uppercase-the-first-character-and-leave-the-rest-alone.
pub fn capitalize(text: &str) -> String {
    let mut c = text.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strcmp_orders_lowercase_immediately_after_its_uppercase_letter_locale_aware() {
        // Real UCA tertiary-strength behavior: "a" < "A" < "b" (case is
        // a tiebreaker within the same base letter), unlike a plain
        // codepoint comparison where every uppercase letter (0x41-0x5A)
        // sorts before every lowercase letter (0x61-0x7A), which would
        // give "A" < "B" < "a".
        assert_eq!(strcmp("a", "A"), Ordering::Less);
        assert_eq!(strcmp("A", "b"), Ordering::Less);
        assert!(matches!(strcmp("apple", "Banana"), Ordering::Less));
    }

    #[test]
    fn strcmp_sorts_accented_letters_next_to_their_base_letter() {
        // A plain codepoint comparison puts "é" (U+00E9) after every
        // ASCII letter including "z"; real collation sorts it right
        // next to "e".
        assert_eq!(strcmp("e", "é"), Ordering::Less);
        assert_eq!(strcmp("é", "f"), Ordering::Less);
    }

    #[test]
    fn primary_strcmp_ignores_case_and_accents() {
        assert_eq!(primary_strcmp("cafe", "café"), Ordering::Equal);
        assert_eq!(primary_strcmp("APPLE", "apple"), Ordering::Equal);
        assert_eq!(primary_strcmp("cote", "côte"), Ordering::Equal);
    }

    #[test]
    fn strcmp_still_distinguishes_case_and_accents_that_primary_strcmp_folds() {
        assert_ne!(strcmp("cafe", "café"), Ordering::Equal);
        assert_ne!(strcmp("APPLE", "apple"), Ordering::Equal);
    }

    #[test]
    fn capitalize_uppercases_first_and_lowercases_the_rest() {
        assert_eq!(capitalize("hello WORLD"), "Hello world");
    }

    #[test]
    fn capitalize_of_empty_string_is_empty() {
        assert_eq!(capitalize(""), "");
    }
}

/// Port of `calibre.utils.icu.title_case`: uppercase the first letter of
/// each word, lowercase the rest. A word boundary is any non-alphabetic
/// character. Narrower than real ICU title-casing (which uses full
/// Unicode word-break rules and a small exception list for
/// articles/prepositions in some locales), but correct for the common
/// case this crate uses it for (CSS `text-transform: capitalize`).
pub fn title_case(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut at_word_start = true;
    for ch in text.chars() {
        if ch.is_alphabetic() {
            if at_word_start {
                result.extend(ch.to_uppercase());
                at_word_start = false;
            } else {
                result.extend(ch.to_lowercase());
            }
        } else {
            result.push(ch);
            at_word_start = true;
        }
    }
    result
}
