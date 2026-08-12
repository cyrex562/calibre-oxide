//! Port of `accurate_page_generator.py`.
//!
//! Parses the decompressed HTML byte stream with a byte-level state
//! machine (matching the Python original's design) to identify "line"
//! positions — paragraph starts + every 70 non-markup chars inside a
//! paragraph. Every 32nd line marks a page.
//!
//! The Python comment claims "every 30 lines" but the code does
//! `range(0, len(lines), 32)`. The 32 wins — preserve the actual
//! behavior, note the doc-comment lie.

use std::path::Path;

use anyhow::Result;

use super::super::i_page_generator::{mobi_html, IPageGenerator};
use super::super::pages::Pages;
use super::fast::FastPageGenerator;

/// Lines per page in the accurate strategy. Python literal `32` from
/// `range(0, len(lines), 32)` — NOT the "32 lines per page and 70
/// characters per line" claim in the surrounding comment (that says
/// 30). We match the code, not the comment.
pub const LINES_PER_PAGE: usize = 32;

/// Characters per line inside a paragraph before a soft-break gets
/// counted as a new line.
pub const CHARS_PER_LINE: u32 = 70;

#[derive(Debug, Default, Clone, Copy)]
pub struct AccuratePageGenerator;

impl AccuratePageGenerator {
    pub const NAME: &'static str = "AccuratePageGenerator";

    pub fn new() -> Self {
        Self
    }
}

impl IPageGenerator for AccuratePageGenerator {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn generate_primary(&self, mobi_file_path: &Path, _real_count: Option<u32>) -> Result<Pages> {
        let html = mobi_html(mobi_file_path)?;
        Ok(Pages::from_arabic_locations(accurate_page_locations(&html)))
    }

    fn generate_fallback(&self, mobi_file_path: &Path, real_count: Option<u32>) -> Result<Pages> {
        FastPageGenerator::new().generate(mobi_file_path, real_count)
    }
}

/// Walk the HTML bytes, emitting a line position for each `<p>` open
/// and each subsequent 70 non-markup chars inside that paragraph. Then
/// keep every 32nd line as a page anchor. Pure function so tests can
/// exercise it without a real MOBI file.
///
/// Byte-level (not char-level) — matches the Python `bytearray`
/// walker exactly. This is safe because the markers we key on
/// (`<`, `>`, `p`, `/`) are all ASCII.
pub fn accurate_page_locations(html: &[u8]) -> Vec<u32> {
    let lines = accurate_line_positions(html);
    lines
        .iter()
        .step_by(LINES_PER_PAGE)
        .map(|&pos| pos as u32)
        .collect()
}

/// Extract just the line positions — every `<p>` open + every 70
/// non-markup chars inside a paragraph. Separated so tests can pin
/// the state machine's behavior independently of the 32-lines-per-page
/// striding.
pub fn accurate_line_positions(html: &[u8]) -> Vec<i64> {
    const SLASH: u8 = b'/';
    const P: u8 = b'p';
    const LT: u8 = b'<';
    const GT: u8 = b'>';

    let mut lines: Vec<i64> = Vec::new();
    // Python: pos starts at -1 and pos += 1 fires at the top of every
    // iteration. Using i64 to preserve the "pos - 2" arithmetic when
    // pos is small.
    let mut pos: i64 = -1;
    let mut in_tag = false;
    let mut in_p = false;
    let mut check_p = false;
    let mut closing = false;
    let mut p_char_count: u32 = 0;

    for &c in html {
        pos += 1;

        if check_p {
            if c == SLASH {
                closing = true;
                continue;
            } else if c == P {
                if closing {
                    in_p = false;
                } else {
                    in_p = true;
                    // Python `lines.append(pos - 2)` — 2 chars back is
                    // where the `<` sits (chars we've seen so far are
                    // `<`, then `p`, so pos points at `p` at index 1
                    // into the tag; pos-2 rewinds past `<`, ready to
                    // step forward again).
                    lines.push(pos - 2);
                }
            }
            check_p = false;
            closing = false;
            continue;
        }

        if c == LT {
            in_tag = true;
            check_p = true;
            continue;
        } else if c == GT {
            in_tag = false;
            check_p = false;
            continue;
        }

        if in_p && !in_tag {
            p_char_count += 1;
            if p_char_count == CHARS_PER_LINE {
                lines.push(pos);
                p_char_count = 0;
            }
        }
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_html_yields_no_lines() {
        assert!(accurate_line_positions(b"").is_empty());
    }

    #[test]
    fn html_without_paragraphs_yields_no_lines() {
        // Text without any <p> opens produces no line anchors.
        assert!(accurate_line_positions(b"just plain text with <div> and <span>").is_empty());
    }

    #[test]
    fn single_paragraph_open_produces_one_line_anchor() {
        // `<p>` at bytes 0..2 (i.e. `<`, `p`, `>` positions 0,1,2).
        // Python emits pos-2 when we see the `p` at pos=1, so line
        // position = -1. This is the Python behavior; preserve it.
        let lines = accurate_line_positions(b"<p>hello");
        assert_eq!(lines, vec![-1]);
    }

    #[test]
    fn closing_p_tag_ends_paragraph_and_stops_counting() {
        // `<p>` opens, some content (below 70 chars), `</p>` closes,
        // then more content (should NOT count toward line breaks).
        let bytes = b"<p>abc</p>defghijklmnopqrstuvwxyz";
        let lines = accurate_line_positions(bytes);
        // Only the opening tag produces an anchor; the trailing text
        // is outside a paragraph.
        assert_eq!(lines, vec![-1]);
    }

    #[test]
    fn seventy_chars_inside_paragraph_produces_soft_line_break() {
        // `<p>` open (1 anchor at pos-2 = 1) + 70 content chars
        // → second anchor at position of the 70th char.
        let content = "x".repeat(70);
        let html = format!("<p>{content}");
        let lines = accurate_line_positions(html.as_bytes());
        assert_eq!(lines.len(), 2, "lines = {:?}", lines);
        // First anchor: `<p>` seen with pos-2 at the `<`.
        assert_eq!(lines[0], -1);
        // Second anchor: the position of the 70th content char.
        // `<p>` = bytes 0,1,2. Content starts at 3. 70th char is at
        // position 3 + 69 = 72.
        assert_eq!(lines[1], 72);
    }

    #[test]
    fn markup_inside_paragraph_does_not_count_toward_line_break() {
        // 35 chars, then `<em>` (4 chars, not counted), then 35 more
        // chars. Total non-markup = 70. Expect ONE soft break.
        let mut html = Vec::new();
        html.extend_from_slice(b"<p>");
        html.extend_from_slice(&b"a".repeat(35));
        html.extend_from_slice(b"<em>");
        html.extend_from_slice(&b"b".repeat(35));
        let lines = accurate_line_positions(&html);
        assert_eq!(lines.len(), 2, "lines = {:?}", lines);
    }

    #[test]
    fn accurate_page_locations_strides_every_lines_per_page() {
        // Fake state: make an html that produces exactly N line
        // positions using paragraph opens. Each `<p>` open contributes
        // one line at pos-2. Between opens we need a close so state
        // resets; use `<p></p>` pairs.
        //
        // With 5 `<p></p>` cycles we'd get 5 line anchors. Only the
        // 0th (index 0) should survive with LINES_PER_PAGE = 32.
        let mut html = Vec::new();
        for _ in 0..5 {
            html.extend_from_slice(b"<p></p>");
        }
        let pages = accurate_page_locations(&html);
        assert_eq!(pages.len(), 1);
    }
}
