//! Cross-validation of the HTMLZ writer's pure functions against
//! calibre's.
//!
//! Two pieces of `oeb2html.py` need only the Python standard library:
//! `prepare_string_for_html`'s escaping and named-entity substitution,
//! and the style-attribute munging the inline-CSS flavour does.
//!
//! The text corpus deliberately avoids entity *references* (`&`
//! followed by a letter or `#`), because `prepare_string_for_xml`
//! resolves those through calibre's own entity table before escaping.
//! That table is ported and tested separately in `html_entities`; what
//! is compared here is everything after it.

#[path = "data/htmlz_vectors.rs"]
mod vectors;

use calibre_ebooks::htmlz::oeb2html::{inline_style_attribute, Oeb2Html};

#[test]
fn text_escaping_matches_calibre_on_every_vector() {
    let mut mismatches = Vec::new();
    for (input, expected) in vectors::TEXT_VECTORS {
        let got = Oeb2Html::prepare_string_for_html(input);
        if got != *expected {
            mismatches.push(format!("{input:?}: rust={got:?} calibre={expected:?}"));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} vectors differ:\n{}",
        mismatches.len(),
        vectors::TEXT_VECTORS.len(),
        mismatches
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn inline_style_munging_matches_calibre_on_every_vector() {
    let mut mismatches = Vec::new();
    for (declared, is_body, expected) in vectors::STYLE_VECTORS {
        let got = inline_style_attribute(declared, *is_body);
        if got != *expected {
            mismatches.push(format!(
                "{declared:?} (is_body={is_body}): rust={got:?} calibre={expected:?}"
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} vectors differ:\n{}",
        mismatches.len(),
        vectors::STYLE_VECTORS.len(),
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
    assert!(vectors::TEXT_VECTORS.len() > 300, "text corpus too small");
    assert!(vectors::STYLE_VECTORS.len() > 400, "style corpus too small");

    let texts: Vec<&str> = vectors::TEXT_VECTORS.iter().map(|(_, e)| *e).collect();
    for entity in [
        "&amp;", "&lt;", "&gt;", "&shy;", "&mdash;", "&ndash;", "&nbsp;",
    ] {
        assert!(
            texts.iter().any(|t| t.contains(entity)),
            "no vector produced {entity}"
        );
    }

    let styles: Vec<&str> = vectors::STYLE_VECTORS.iter().map(|(_, _, e)| *e).collect();
    assert!(styles.iter().any(|s| s.is_empty()), "no empty style");
    assert!(styles
        .iter()
        .any(|s| s.contains("page-break-before: always")));
    // The quote swap only shows when the declaration had a double quote.
    assert!(
        styles.iter().any(|s| s.contains('\'')),
        "no vector exercised the quote swap"
    );
}
