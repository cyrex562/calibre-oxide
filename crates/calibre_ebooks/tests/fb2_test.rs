//! Cross-validation of the FB2 writer's pure functions against
//! calibre's.
//!
//! `clean_text` and `base64_decode` need nothing but the Python
//! standard library, so calibre's own code can be run directly. The
//! vectors are its output over a token-driven corpus of FB2 markup and
//! over base64 payloads including the malformed ones FB2 files carry
//! in practice.

#[path = "data/fb2_b64_vectors.rs"]
mod b64_vectors;
#[path = "data/fb2_clean_vectors.rs"]
mod clean_vectors;

use calibre_ebooks::fb2::base64_decode;
use calibre_ebooks::fb2::fb2ml::clean_text;

#[test]
fn clean_text_matches_calibre_on_every_vector() {
    let mut mismatches = Vec::new();
    for (input, flag, expected) in clean_vectors::CLEAN_VECTORS {
        let got = clean_text(input, *flag);
        if got != *expected {
            mismatches.push(format!(
                "{input:?} (insert_blank_line={flag})\n     rust: {got:?}\n  calibre: {expected:?}"
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} vectors differ:\n{}",
        mismatches.len(),
        clean_vectors::CLEAN_VECTORS.len(),
        mismatches
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn base64_decode_matches_calibre_on_every_vector() {
    let mut mismatches = Vec::new();
    for (input, expected) in b64_vectors::B64_VECTORS {
        let got = base64_decode(input);
        if got != *expected {
            mismatches.push(format!("{input:?}: rust={got:?} calibre={expected:?}"));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} vectors differ:\n{}",
        mismatches.len(),
        b64_vectors::B64_VECTORS.len(),
        mismatches
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn the_corpora_reach_the_interesting_branches() {
    assert!(
        clean_vectors::CLEAN_VECTORS.len() > 3000,
        "clean corpus too small"
    );
    assert!(
        b64_vectors::B64_VECTORS.len() > 500,
        "base64 corpus too small"
    );

    let outputs: Vec<&str> = clean_vectors::CLEAN_VECTORS
        .iter()
        .map(|(_, _, e)| *e)
        .collect();
    for (needle, what) in [
        ("<empty-line/>", "an empty-line"),
        ("</p>\n<p>", "paragraphs split onto lines"),
        ("<section>\n", "a tidied section"),
        ("</title>", "a title"),
    ] {
        assert!(
            outputs.iter().any(|o| o.contains(needle)),
            "no vector produced {what}"
        );
    }
    // The non-breaking-space case is the one that distinguishes
    // calibre's ASCII-only pattern from a Unicode one.
    assert!(
        clean_vectors::CLEAN_VECTORS
            .iter()
            .any(|(i, _, _)| i.contains('\u{a0}')),
        "no vector exercised the ASCII-whitespace pattern"
    );
}
