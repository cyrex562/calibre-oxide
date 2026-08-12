//! Cross-validation of `filter_name` against calibre's implementation.
//!
//! `filter_name` in `old_src/src/calibre/ebooks/epub/pages.py` needs
//! nothing but Python's `re`, so it can be run directly. The vectors
//! below are its output over a corpus of page labels — hand-picked
//! edge cases, every combination of a small grammar, and 1200 random
//! strings built from the tokens that matter (`page`, digits, roman
//! numerals, separators).
//!
//! 925 vectors in all.

#[path = "data/epub_filter_name_vectors.rs"]
mod vectors;

use calibre_ebooks::epub::pages::filter_name;

#[test]
fn filter_name_matches_calibre_on_every_vector() {
    let mut mismatches = Vec::new();
    for (input, expected) in vectors::VECTORS {
        let got = filter_name(input);
        if got != *expected {
            mismatches.push(format!("{input:?}: rust={got:?} calibre={expected:?}"));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} vectors differ:\n{}",
        mismatches.len(),
        vectors::VECTORS.len(),
        mismatches.join("\n")
    );
}

#[test]
fn the_corpus_actually_exercises_the_interesting_branches() {
    // A vector set that never reached the numeric or roman branch
    // would pass vacuously.
    let outputs: Vec<&str> = vectors::VECTORS.iter().map(|(_, e)| *e).collect();
    assert!(vectors::VECTORS.len() > 800, "corpus is too small");
    assert!(outputs.contains(&"42"), "no plain-number case");
    assert!(outputs.contains(&"xiv"), "no roman-numeral case");
    assert!(
        outputs.iter().any(|o| o.starts_with(' ')),
        "no leading-space case"
    );
    assert!(outputs.iter().any(|o| o.is_empty()), "no empty case");
}
