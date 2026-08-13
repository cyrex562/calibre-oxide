//! Cross-validation of the HTML input port against calibre's.
//!
//! Three pieces of `ebooks/html` need only the Python standard library,
//! so calibre's own code can be run directly:
//!
//! - `LINK_PAT`'s href extraction, including which of its three
//!   quoting alternatives wins,
//! - `Link.url_to_local_path`, whose query/params re-attachment is the
//!   fiddliest part of the module,
//! - `to_zip.parse_my_settings`, which has to read both the JSON and
//!   the legacy `encoding|bf` forms.

#[path = "data/html_vectors.rs"]
mod vectors;

use calibre_ebooks::html::input::{find_links_in, Link};
use calibre_ebooks::html::to_zip::parse_settings;
use std::path::Path;

#[test]
fn link_extraction_matches_calibre_on_every_vector() {
    let mut mismatches = Vec::new();
    for (src, expected) in vectors::LINK_VECTORS {
        let got = find_links_in(src);
        let want: Vec<String> = expected.iter().map(|s| (*s).to_string()).collect();
        if got != want {
            mismatches.push(format!("{src:?}\n     rust: {got:?}\n  calibre: {want:?}"));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} vectors differ:\n{}",
        mismatches.len(),
        vectors::LINK_VECTORS.len(),
        mismatches
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn path_resolution_matches_calibre_on_every_vector() {
    let mut mismatches = Vec::new();
    for (url, base, expected) in vectors::PATH_VECTORS {
        let link = Link::new(url, Path::new(base));
        let got = link
            .path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        if got != *expected {
            mismatches.push(format!(
                "{url:?} against {base:?}: rust={got:?} calibre={expected:?}"
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} vectors differ:\n{}",
        mismatches.len(),
        vectors::PATH_VECTORS.len(),
        mismatches.join("\n")
    );
}

#[test]
fn settings_parsing_matches_calibre_on_every_vector() {
    for (raw, encoding, breadth_first, allow_outside) in vectors::SETTING_VECTORS {
        let got = parse_settings(raw);
        assert_eq!(
            got.encoding.unwrap_or_default(),
            *encoding,
            "encoding for {raw:?}"
        );
        assert_eq!(
            got.breadth_first, *breadth_first,
            "breadth_first for {raw:?}"
        );
        assert_eq!(
            got.allow_local_files_outside_root, *allow_outside,
            "allow_local_files_outside_root for {raw:?}"
        );
    }
}

#[test]
fn the_corpora_reach_the_interesting_branches() {
    assert!(vectors::LINK_VECTORS.len() > 600, "link corpus too small");
    // All three quoting styles, and a source with no links at all.
    assert!(vectors::LINK_VECTORS
        .iter()
        .any(|(s, _)| s.contains("href='")));
    assert!(vectors::LINK_VECTORS
        .iter()
        .any(|(s, l)| s.contains("href=a") && !l.is_empty()));
    assert!(vectors::LINK_VECTORS.iter().any(|(_, l)| l.is_empty()));
    // Paths with a query, a fragment, params, an absolute path, and a
    // remote URL that resolves to nothing.
    assert!(vectors::PATH_VECTORS
        .iter()
        .any(|(u, _, _)| u.contains('?')));
    assert!(vectors::PATH_VECTORS
        .iter()
        .any(|(u, _, _)| u.contains(';')));
    assert!(vectors::PATH_VECTORS.iter().any(|(_, _, p)| p.is_empty()));
    assert!(vectors::PATH_VECTORS
        .iter()
        .any(|(_, _, p)| p.contains("%") || p.contains(' ')));
}
