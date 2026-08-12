//! Port of `exact_page_generator.py`.

use std::path::Path;

use anyhow::{anyhow, Result};

use super::super::i_page_generator::{mobi_html_length, IPageGenerator};
use super::super::pages::Pages;
use super::fast::FastPageGenerator;

#[derive(Debug, Default, Clone, Copy)]
pub struct ExactPageGenerator;

impl ExactPageGenerator {
    pub const NAME: &'static str = "ExactPageGenerator";

    pub fn new() -> Self {
        Self
    }
}

impl IPageGenerator for ExactPageGenerator {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn generate_primary(&self, mobi_file_path: &Path, real_count: Option<u32>) -> Result<Pages> {
        let real_count = real_count.ok_or_else(|| {
            // The Python original silently `int(text_length // real_count)` with
            // `real_count=None` would raise TypeError. Prefer a typed error.
            anyhow!("ExactPageGenerator requires a page count (real_count)")
        })?;
        if real_count == 0 {
            return Err(anyhow!("ExactPageGenerator: real_count must be > 0"));
        }
        let text_length = mobi_html_length(mobi_file_path)?;
        Ok(Pages::from_arabic_locations(exact_page_locations(
            text_length,
            real_count,
        )))
    }

    fn generate_fallback(&self, mobi_file_path: &Path, real_count: Option<u32>) -> Result<Pages> {
        FastPageGenerator::new().generate(mobi_file_path, real_count)
    }
}

/// Pure function: divide `text_length` by `real_count` to get
/// `chars_per_page`, then emit `0, chars_per_page, 2*chars_per_page, ...`
/// clamped at `real_count` entries (Python `pages[:real_count]`).
pub fn exact_page_locations(text_length: u32, real_count: u32) -> Vec<u32> {
    debug_assert!(real_count > 0, "caller guarantees > 0");
    if text_length == 0 {
        // Python `while count < 0` never enters — no pages. Preserve.
        return Vec::new();
    }
    let chars_per_page = text_length / real_count;
    if chars_per_page == 0 {
        // Python `chars_per_page = 0` would infinite-loop; return
        // exactly one page. This is a real edge case for very short
        // documents.
        return vec![0];
    }
    let mut out = Vec::with_capacity(real_count as usize);
    let mut count = 0u32;
    while count < text_length {
        out.push(count);
        match count.checked_add(chars_per_page) {
            Some(n) => count = n,
            None => break,
        }
    }
    // Rounding can produce extras — clamp per Python's
    // `pages = pages[:real_count]`.
    if out.len() > real_count as usize {
        out.truncate(real_count as usize);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evenly_divisible_case() {
        // 1000 chars / 10 pages = 100 chars/page.
        // Loop: 0, 100, 200, ..., 900. Ten entries. No truncation.
        let got = exact_page_locations(1000, 10);
        assert_eq!(got.len(), 10);
        assert_eq!(got[0], 0);
        assert_eq!(got[9], 900);
    }

    #[test]
    fn truncates_rounding_extras_to_real_count() {
        // 1005 chars / 10 pages = 100 chars/page (integer div).
        // Loop: 0, 100, ..., 1000 = 11 entries. Truncate to 10.
        let got = exact_page_locations(1005, 10);
        assert_eq!(got.len(), 10);
        assert_eq!(*got.last().unwrap(), 900);
    }

    #[test]
    fn tiny_document_short_circuits_to_single_page() {
        // 5 chars / 10 pages = 0 chars/page — Python would infinite-loop.
        // We short-circuit to a single page.
        assert_eq!(exact_page_locations(5, 10), vec![0]);
    }

    #[test]
    fn zero_length_document_yields_no_pages() {
        // Python `while count < 0` never enters — no pages.
        assert_eq!(exact_page_locations(0, 10), Vec::<u32>::new());
    }

    #[test]
    fn real_count_of_one_yields_single_page() {
        assert_eq!(exact_page_locations(1000, 1), vec![0]);
    }
}
