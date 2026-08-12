//! Port of `pagebreak_page_generator.py`.
//!
//! Anchors pages to explicit `<*pagebreak*/>` markers in the HTML.
//! This is a byte-level regex against the raw HTML — matches
//! `<div class="mbp_pagebreak" />`, `<hr class="pagebreak"/>`, and
//! anything else containing the substring `pagebreak` inside a tag.

use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use regex::bytes::Regex;

use super::super::i_page_generator::{mobi_html, IPageGenerator};
use super::super::pages::Pages;
use super::fast::FastPageGenerator;

#[derive(Debug, Default, Clone, Copy)]
pub struct PagebreakPageGenerator;

impl PagebreakPageGenerator {
    pub const NAME: &'static str = "PagebreakPageGenerator";

    pub fn new() -> Self {
        Self
    }
}

impl IPageGenerator for PagebreakPageGenerator {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn generate_primary(&self, mobi_file_path: &Path, _real_count: Option<u32>) -> Result<Pages> {
        let html = mobi_html(mobi_file_path)?;
        Ok(Pages::from_arabic_locations(pagebreak_page_locations(&html)?))
    }

    fn generate_fallback(&self, mobi_file_path: &Path, real_count: Option<u32>) -> Result<Pages> {
        FastPageGenerator::new().generate(mobi_file_path, real_count)
    }
}

/// The regex from Python: `br'<[^>]*pagebreak[^>]*>'`. Compiled once
/// per process via `OnceLock` — the pattern is fixed and the compile
/// isn't cheap enough to redo per file.
fn pagebreak_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"<[^>]*pagebreak[^>]*>").expect("static pagebreak regex is valid"))
}

/// Pure function: return the byte offset of the end of each
/// `<...pagebreak...>` match. Matches the Python `m.end()` return.
pub fn pagebreak_page_locations(html: &[u8]) -> Result<Vec<u32>> {
    let re = pagebreak_regex();
    let mut out: Vec<u32> = Vec::new();
    for m in re.find_iter(html) {
        let end = u32::try_from(m.end())
            .context("pagebreak position exceeds u32 range (>4 GB MOBI)")?;
        out.push(end);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_pagebreak_yields_empty() {
        assert!(pagebreak_page_locations(b"<div>hello</div>").unwrap().is_empty());
    }

    #[test]
    fn mbp_pagebreak_matches() {
        let html = br#"<div class="mbp_pagebreak" />body text"#;
        let locs = pagebreak_page_locations(html).unwrap();
        assert_eq!(locs.len(), 1);
        // The match ends at the closing `>` — the byte immediately
        // after the tag. That's where the "next page" begins.
        assert_eq!(locs[0], html.iter().position(|&b| b == b'>').unwrap() as u32 + 1);
    }

    #[test]
    fn multiple_pagebreak_variants_all_matched() {
        let html: Vec<u8> = [
            b"start" as &[u8],
            br#"<hr class="pagebreak"/>"#,
            b"middle",
            br#"<div class="mbp_pagebreak" />"#,
            b"end",
        ]
        .concat();
        let locs = pagebreak_page_locations(&html).unwrap();
        assert_eq!(locs.len(), 2);
        // The two locations must be strictly increasing.
        assert!(locs[1] > locs[0]);
    }

    #[test]
    fn substring_pagebreak_outside_tag_does_not_match() {
        // The regex requires `<...pagebreak...>` — a bare occurrence
        // of the word "pagebreak" in body text must NOT match.
        assert!(
            pagebreak_page_locations(b"the word pagebreak appears here")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn location_is_end_of_match_not_start() {
        // Regression guard against a common bug: emitting `m.start()`
        // instead of `m.end()` would put the anchor BEFORE the
        // pagebreak marker, off by a page.
        let html = br#"prefix<hr class="pagebreak"/>suffix"#;
        let locs = pagebreak_page_locations(html).unwrap();
        assert_eq!(locs.len(), 1);
        // The anchor byte should be the `s` of "suffix" (i.e.,
        // strictly greater than the position of the `<` that starts
        // the tag).
        let tag_start = html.iter().position(|&b| b == b'<').unwrap() as u32;
        assert!(locs[0] > tag_start);
    }
}
