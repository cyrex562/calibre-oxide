//! Port of `fast_page_generator.py`.

use std::path::Path;

use anyhow::{anyhow, Result};

use super::super::i_page_generator::{mobi_html_length, IPageGenerator};
use super::super::pages::Pages;

/// 2300 characters of uncompressed text per page. The number is a
/// literal from the Python original — its comment explains: a test
/// book was chosen and characters per page counted; the number was
/// rounded to 2240 then 60 characters of markup added to give 2300.
///
/// Uses uncompressed text length (readable directly from the MOBI
/// header) rather than decompressing + parsing — favors speed over
/// accuracy.
pub const FAST_CHARS_PER_PAGE: u32 = 2300;

#[derive(Debug, Default, Clone, Copy)]
pub struct FastPageGenerator;

impl FastPageGenerator {
    pub const NAME: &'static str = "FastPageGenerator";

    pub fn new() -> Self {
        Self
    }
}

impl IPageGenerator for FastPageGenerator {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn generate_primary(&self, mobi_file_path: &Path, _real_count: Option<u32>) -> Result<Pages> {
        let text_length = mobi_html_length(mobi_file_path)?;
        Ok(Pages::from_arabic_locations(fast_page_locations(text_length)))
    }

    fn generate_fallback(
        &self,
        _mobi_file_path: &Path,
        _real_count: Option<u32>,
    ) -> Result<Pages> {
        // Matches the Python `raise Exception('Fast calculation
        // impossible.')`. The `IPageGenerator::generate` orchestrator
        // preserves this error unchanged for the fast generator (does
        // not swallow into a fallback loop) — see i_page_generator.rs.
        Err(anyhow!("Fast calculation impossible."))
    }
}

/// Pure function: given a text length in bytes, emit page start
/// offsets `0, 2300, 4600, ...` up to (but not including) the text
/// length. Extracted from `generate_primary` so it's testable
/// without a MOBI file.
pub fn fast_page_locations(text_length: u32) -> Vec<u32> {
    let mut out = Vec::with_capacity((text_length / FAST_CHARS_PER_PAGE) as usize + 1);
    let mut count = 0u32;
    while count < text_length {
        out.push(count);
        count = count.saturating_add(FAST_CHARS_PER_PAGE);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_yields_no_pages() {
        assert_eq!(fast_page_locations(0), Vec::<u32>::new());
    }

    #[test]
    fn one_page_for_text_shorter_than_page_size() {
        // Any nonzero length under 2300 must produce exactly one page
        // at offset 0 — the Python `while count < text_length` loop
        // enters at count=0.
        assert_eq!(fast_page_locations(1), vec![0]);
        assert_eq!(fast_page_locations(2299), vec![0]);
    }

    #[test]
    fn exact_multiple_yields_expected_count() {
        // 2300 → one page (loop enters at 0, exits before 2300).
        assert_eq!(fast_page_locations(2300), vec![0]);
        // 4600 → two pages.
        assert_eq!(fast_page_locations(4600), vec![0, 2300]);
    }

    #[test]
    fn non_multiple_yields_ceil_pages() {
        // 5000 → 0, 2300, 4600 (loop: 4600 < 5000, then 6900 >= 5000).
        assert_eq!(fast_page_locations(5000), vec![0, 2300, 4600]);
    }

    #[test]
    fn saturating_add_prevents_overflow_at_u32_max() {
        // If someone passes an absurdly large text_length, the loop
        // must terminate rather than panic on overflow.
        let out = fast_page_locations(u32::MAX);
        assert_eq!(out.first().copied(), Some(0));
        assert!(out.len() >= 1);
    }

    #[test]
    fn name_matches_orchestrator_special_case() {
        // The IPageGenerator::generate orchestrator special-cases the
        // name "FastPageGenerator" to propagate errors instead of
        // falling back. If we ever rename this, the fast-generator
        // error-propagation behavior in i_page_generator.rs also
        // needs updating.
        assert_eq!(FastPageGenerator::new().name(), "FastPageGenerator");
    }
}
