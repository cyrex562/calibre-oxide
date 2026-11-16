//! Finding the same PDF saved differently (tranche 9).
//!
//! Byte hashes, identifiers and titles find a duplicate that *says* it is
//! one. Two kinds are invisible to all of them, and both are common in a big
//! mixed collection:
//!
//! * **scan vs OCR**: the same scanned book once as image-only pages and once
//!   with an OCR text layer (usually re-encoded as well);
//! * **optimizer re-saves**: the same PDF put through a compressor
//!   (downsampled images, re-encoded streams, linearised, metadata stripped).
//!
//! They often differ in title and author too (or have none: a title taken from
//! the filename), so they are found from the *pages*, not the metadata.
//!
//! **Nothing here is ported from calibre.** Calibre has no such feature (its
//! duplicate finder is a title/author plugin and `find_identical_books`). This
//! module is oxide-specific.
//!
//! # Fingerprint
//!
//! One [`PdfFingerprint`] per distinct file content (keyed by the BLAKE3 the
//! scan already recorded, so identical files share a row and a moved file
//! needs nothing recomputed), computed once from the file on disk by
//! [`fingerprint_file`] and cached in the checksum sidecar (see
//! [`crate::checksums::ChecksumStore::record_pdf_fingerprint`]):
//!
//! * `page_count`;
//! * a 64-bit perceptual hash ([`page_hash`]) of up to [`SAMPLE_PAGES`] = 8
//!   sampled pages: the first three, the last three and two in the middle
//!   ([`sample_indices`]; every page of a document of 8 or fewer). The
//!   front/back anchoring is deliberate: a scan that gained or lost a cover
//!   or a blank page shifts the *front* of the book but not its end, so each
//!   end can be aligned on its own (see "Verify");
//! * whether there is a **text layer** (average of at least [`MIN_TEXT_CHARS_PER_PAGE`]
//!   letters/digits a page over the first [`TEXT_PAGES`] pages), and a
//!   [`MINHASH_LEN`]-slot MinHash of the word 3-shingles of that text
//!   ([`minhash`]).
//!
//! ## The page hash
//!
//! The page is rendered by PDFium to a grayscale image with a long side of
//! [`RENDER_LONG_SIDE`] = 256 px (`rasterize::sample_pdf_from_path`), then
//! [`page_hash`] makes it comparable across the differences between a scan
//! and its copy:
//!
//! 1. **ink map**: the page is averaged into a fixed 48x64 grid (about 25
//!    pixels a cell), so scanner grain and JPEG noise, a few gray levels a
//!    cell, vanish under text, which is tens. "Ink" is a cell 40 or more gray
//!    levels below the paper (the 90th-percentile cell);
//! 2. **trim margins**: a scanner's dark border (an outermost row or column
//!    more than 100 below the paper) is peeled off, a cell at a time and one
//!    more, then the page is cropped to its ink (a row or column needs two ink
//!    cells, so a speck of dust is not content). A shifted, bordered or
//!    differently framed scan crops to the same text block;
//! 3. area-average the crop to 32x32 and take its DCT; the 64 lowest-
//!    frequency AC coefficients, each against their median, are the 64 bits
//!    (a classic pHash, written here on plain slices: no new dependency).
//!
//! A page whose ink is under 1% of its cells is **blank** (no hash, never
//! compared): a white page, a uniformly dark one, paper grain, and a page
//! holding only a page number all land here.
//!
//! # Matching
//!
//! Never all-pairs ([`find_pairs`]).
//!
//! **Candidates** come from multi-index hashing on the page hashes: each
//! non-blank sampled page's hash is split into four 16-bit chunks and
//! indexed under `(page_count, chunk number, chunk value)`. A fingerprint
//! looks up its own chunks under page counts `n-2..=n+2` and a pair is a
//! candidate when at least [`MIN_CANDIDATE_PAGES`] = 2 of its pages hit (one
//! for a document with at most two usable pages). A posting list longer than
//! [`MAX_POSTING`] is a chunk value that says nothing and is skipped. Chunk
//! equality is a *filter*, not a proof: two page hashes within the match
//! distance share a chunk only with probability roughly 35% at distance 8
//! (about 90% at 4), so a pair is missed only if it fails that on every page:
//! at 6 matching pages, under 10% miss at distance 8, and real re-renders of
//! one source image are usually far closer than that. Verification, not the
//! index, decides.
//!
//! **Verify** ([`visual_match`]) aligns the sample pages. Equal page counts
//! compare the same positions. Unequal counts (a cover added, a trailing
//! blank dropped, up to [`MAX_PAGE_DIFF`] = 2) try every shift of the front
//! frame, with the end frame shifted by the page-count difference less that,
//! and keep the alignment with the most matching pages. A pair of pages
//! match at Hamming distance <= [`PAGE_MATCH_DISTANCE`] = 10 (the test
//! `same_pages_are_close_and_different_pages_are_far` prints the evidence: on
//! 400 synthetic scans, shifted/bordered/JPEG/downsampled copies of 80 pages,
//! same-page distances had median 4, p90 8, p99 12, max 14, and different
//! pages never came closer than 14 bits, p10 20, median 28. Those pages are
//! synthetic, not PDFium renders of real books: see the PR notes). A pair of
//! documents matches visually when at least [`MIN_COMPARED`] = 3 non-blank
//! pages were compared and at least 75% of them match. A document of one or
//! two pages needs equal page counts and *every* page to match, and gets a
//! confidence penalty.
//!
//! **Classify** ([`classify`]), given a visual match:
//!
//! | Evidence | Kind | Confidence |
//! | --- | --- | --- |
//! | exactly one side has a text layer | `pdf_ocr_variant` | 0.80 |
//! | neither has text | `pdf_resave` | 0.85 |
//! | both have text, MinHash Jaccard >= 0.8 | `pdf_resave` | 0.85 |
//! | both have text, 0.3 <= Jaccard < 0.8 | `pdf_ocr_variant` (two OCR runs) | 0.75 |
//! | both have text, Jaccard < 0.3 | *no match*: the same look but other words is a different book (a series template) | |
//!
//! With no visual match, two text-bearing PDFs of at least
//! [`TEXT_MATCH_MIN_CHARS`] letters whose Jaccard is >= 0.9 and whose page
//! counts are within two are `pdf_text_match` (0.60): the same words
//! re-typeset. These come from a second index, MinHash LSH (16 bands of 8).
//!
//! # What can go wrong
//!
//! * **Pages that look alike.** A 64-bit pHash of a text page is coarse; two
//!   books from one publisher template can land close. The guards are the 75%
//!   and 3-page rules, the blank rule, the text-disagreement rule above, and
//!   that the alignment is chosen by matches, so the number it needs is
//!   already a best case.
//! * **Middle pages are barely sampled** (two). A document whose two ends
//!   match and whose middle is different is reported; it is a near-copy at
//!   worst.
//! * A file changed on disk since it was hashed is fingerprinted as whatever
//!   it now contains, under the old hash. The library check already flags such
//!   a file.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, bail, Result};
use serde::Serialize;

use crate::cache::Cache;
use crate::scan_pipeline::{format_file_path, parallel_map, JobControl, PipelineOptions};

/// Bump when the hash, sampling or MinHash changes: every row with another
/// version is recomputed.
pub const FINGERPRINT_VERSION: u32 = 1;
/// Pages hashed per PDF.
pub const SAMPLE_PAGES: usize = 8;
/// Long side of the render the hash is made from.
pub const RENDER_LONG_SIDE: u32 = 256;
/// Pages (from the start) whose text is read.
pub const TEXT_PAGES: usize = 48;
/// A text layer is at least this many letters or digits a page, on average.
pub const MIN_TEXT_CHARS_PER_PAGE: u32 = 20;
/// Slots in a text MinHash.
pub const MINHASH_LEN: usize = 128;
/// Hamming distance, in bits of 64, at or under which two pages are the same.
pub const PAGE_MATCH_DISTANCE: u32 = 10;
/// Fraction of compared pages that must match.
pub const MATCH_FRACTION: f32 = 0.75;
/// Non-blank pages that must be compared (documents of up to two pages
/// instead need every page, and equal counts).
pub const MIN_COMPARED: u32 = 3;
/// Page-count difference tolerated.
pub const MAX_PAGE_DIFF: u32 = 2;
/// Distinct sampled pages that must hit the index for a pair to be verified.
pub const MIN_CANDIDATE_PAGES: u32 = 2;
/// An index posting list longer than this is ignored.
pub const MAX_POSTING: usize = 200;
/// Jaccard at or above which two text layers are the same text.
pub const SAME_TEXT_JACCARD: f32 = 0.8;
/// Jaccard below which two text layers are different books.
pub const DIFFERENT_TEXT_JACCARD: f32 = 0.3;
/// Jaccard for a text-only match.
pub const TEXT_MATCH_JACCARD: f32 = 0.9;
/// Letters a text-only match needs on each side.
pub const TEXT_MATCH_MIN_CHARS: u32 = 2000;

pub const CONFIDENCE_RESAVE: f32 = 0.85;
pub const CONFIDENCE_OCR: f32 = 0.80;
pub const CONFIDENCE_OCR_TWO_LAYERS: f32 = 0.75;
pub const CONFIDENCE_TEXT: f32 = 0.60;
/// Multiplier for a document of one or two pages.
pub const TINY_PENALTY: f32 = 0.85;

// ---------------------------------------------------------------------
// The page hash
// ---------------------------------------------------------------------

/// One sampled page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageHash {
    /// 0-based page number.
    pub index: u32,
    /// Zero when `blank`.
    pub hash: u64,
    pub blank: bool,
}

/// How many bits differ.
pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// The ink map's grid, and what counts as ink in it. A cell is the mean of
/// about 25 pixels of a 256 px render; "deficit" is how far it is below the
/// paper (the 90th percentile cell).
const GRID_W: usize = 48;
const GRID_H: usize = 64;
const INK_DEFICIT: f32 = 40.0;
/// A border line (the outermost row or column) darker than this is a scanner
/// edge, not content.
const BORDER_DEFICIT: f32 = 100.0;
const BORDER_MAX_CELLS: usize = 6;
/// A page with less ink than this share of its cells is blank.
const MIN_INK_PERCENT: usize = 1;

/// `(u, v)` of the 64 DCT coefficients that make the hash, low frequency
/// first, DC excluded.
fn coefficient_order() -> Vec<(usize, usize)> {
    let mut all: Vec<(usize, usize)> = (0..=8).flat_map(|u| (0..=8).map(move |v| (u, v))).filter(|&(u, v)| u + v >= 1).collect();
    all.sort_by_key(|&(u, v)| (u + v, u));
    all.truncate(64);
    all
}

/// Hashes one grayscale page (one byte a pixel, row-major). `None` for a
/// blank page. See the module docs for what it does.
pub fn page_hash(gray: &[u8], width: u32, height: u32) -> Option<u64> {
    analyse_page(gray, width, height).map(|a| a.0)
}

/// [`page_hash`] and the crop it hashed `(x0, x1, y0, y1)` in 2x2-downsampled
/// pixels, for the tests that tune it.
pub(crate) fn analyse_page(gray: &[u8], width: u32, height: u32) -> Option<(u64, (usize, usize, usize, usize))> {
    let (w, h) = (width as usize, height as usize);
    if w < GRID_W || h < GRID_H || gray.len() < w * h {
        return None;
    }

    // An ink map: the mean of each cell of a fixed grid, against the paper.
    // Averaging over a cell of ~25 pixels is what makes it robust: scanner and
    // JPEG noise is a few gray levels per cell, text is tens.
    let mut cells = vec![0f32; GRID_W * GRID_H];
    for cy in 0..GRID_H {
        let (y0, y1) = (cy * h / GRID_H, ((cy + 1) * h / GRID_H).max(cy * h / GRID_H + 1));
        for cx in 0..GRID_W {
            let (x0, x1) = (cx * w / GRID_W, ((cx + 1) * w / GRID_W).max(cx * w / GRID_W + 1));
            let mut sum = 0u32;
            for y in y0..y1 {
                sum += gray[y * w + x0..y * w + x1].iter().map(|&v| v as u32).sum::<u32>();
            }
            cells[cy * GRID_W + cx] = sum as f32 / ((y1 - y0) * (x1 - x0)) as f32;
        }
    }
    let mut sorted = cells.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let paper = sorted[sorted.len() * 9 / 10];
    let deficit: Vec<f32> = cells.iter().map(|v| (paper - v).max(0.0)).collect();

    // Peel a dark scanner border: while the outermost line is mostly very
    // dark, drop it (and one more, for the sliver of a cell it only partly
    // covers).
    let (mut l, mut r, mut t, mut b) = (0usize, GRID_W, 0usize, GRID_H);
    let mut peeled = [false; 4];
    for _ in 0..BORDER_MAX_CELLS {
        let mean = |cells: &mut dyn Iterator<Item = usize>| -> f32 {
            let (mut sum, mut n) = (0f32, 0f32);
            for i in cells {
                sum += deficit[i];
                n += 1.0;
            }
            if n > 0.0 {
                sum / n
            } else {
                0.0
            }
        };
        let mut moved = false;
        if r > l + 2 && b > t + 2 {
            if mean(&mut (l..r).map(|x| t * GRID_W + x)) > BORDER_DEFICIT {
                t += 1;
                peeled[0] = true;
                moved = true;
            }
            if mean(&mut (l..r).map(|x| (b - 1) * GRID_W + x)) > BORDER_DEFICIT {
                b -= 1;
                peeled[1] = true;
                moved = true;
            }
            if mean(&mut (t..b).map(|y| y * GRID_W + l)) > BORDER_DEFICIT {
                l += 1;
                peeled[2] = true;
                moved = true;
            }
            if mean(&mut (t..b).map(|y| y * GRID_W + r - 1)) > BORDER_DEFICIT {
                r -= 1;
                peeled[3] = true;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    if peeled[0] {
        t += 1;
    }
    if peeled[1] {
        b -= 1;
    }
    if peeled[2] {
        l += 1;
    }
    if peeled[3] {
        r -= 1;
    }
    if r <= l + 2 || b <= t + 2 {
        return None;
    }

    // Where the ink is.
    let ink = |x: usize, y: usize| deficit[y * GRID_W + x] > INK_DEFICIT;
    let total_ink = (t..b).map(|y| (l..r).filter(|&x| ink(x, y)).count()).sum::<usize>();
    if total_ink * 100 < MIN_INK_PERCENT * GRID_W * GRID_H {
        return None;
    }
    // A row or column needs two ink cells to count: a speck is not content.
    let rows: Vec<usize> = (t..b).filter(|&y| (l..r).filter(|&x| ink(x, y)).count() >= 2).collect();
    let cols: Vec<usize> = (l..r).filter(|&x| (t..b).filter(|&y| ink(x, y)).count() >= 2).collect();
    let (&c0, &c1) = (cols.first()?, cols.last()?);
    let (&r0, &r1) = (rows.first()?, rows.last()?);
    let (c0, c1) = widen(c0, c1 + 1, GRID_W);
    let (r0, r1) = widen(r0, r1 + 1, GRID_H);

    // The crop, in pixels, to 32x32 by area average.
    let (px0, px1) = (c0 * w / GRID_W, (c1 * w / GRID_W).max(c0 * w / GRID_W + 1));
    let (py0, py1) = (r0 * h / GRID_H, (r1 * h / GRID_H).max(r0 * h / GRID_H + 1));
    const N: usize = 32;
    let mut block = [[0f32; N]; N];
    for (ty, line) in block.iter_mut().enumerate() {
        let sy0 = py0 + ty * (py1 - py0) / N;
        let sy1 = (py0 + (ty + 1) * (py1 - py0) / N).max(sy0 + 1).min(h);
        for (tx, cell) in line.iter_mut().enumerate() {
            let sx0 = px0 + tx * (px1 - px0) / N;
            let sx1 = (px0 + (tx + 1) * (px1 - px0) / N).max(sx0 + 1).min(w);
            let mut sum = 0u32;
            let mut n = 0u32;
            for sy in sy0..sy1 {
                for &v in &gray[sy * w + sx0..sy * w + sx1] {
                    sum += v as u32;
                    n += 1;
                }
            }
            *cell = if n > 0 { sum as f32 / n as f32 } else { 0.0 };
        }
    }

    // Separable DCT, only the coefficients that can be wanted (u, v <= 8).
    let cos: Vec<[f32; N]> = (0..=8).map(|u| std::array::from_fn(|x| (((2 * x + 1) as f32) * (u as f32) * std::f32::consts::PI / (2.0 * N as f32)).cos())).collect();
    let mut rows_dct = [[0f32; 9]; N];
    for y in 0..N {
        for (u, c) in cos.iter().enumerate() {
            rows_dct[y][u] = (0..N).map(|x| block[y][x] * c[x]).sum();
        }
    }
    let coefficient = |u: usize, v: usize| -> f32 { (0..N).map(|y| rows_dct[y][u] * cos[v][y]).sum() };
    let order = coefficient_order();
    let values: Vec<f32> = order.iter().map(|&(u, v)| coefficient(u, v)).collect();
    let mut sorted = values.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let median = (sorted[31] + sorted[32]) / 2.0;
    let mut hash = 0u64;
    for (bit, value) in values.iter().enumerate() {
        if *value > median {
            hash |= 1 << bit;
        }
    }
    Some((hash, (px0, px1, py0, py1)))
}

/// A crop narrower than a quarter of the page (a single line of text) is
/// widened to that, around its centre, so the hash is of a page-shaped
/// region and not of a stretched sliver.
fn widen(lo: usize, hi: usize, size: usize) -> (usize, usize) {
    let min = (size / 4).max(4).min(size);
    if hi - lo >= min {
        return (lo, hi);
    }
    let centre = (lo + hi) / 2;
    let start = centre.saturating_sub(min / 2).min(size - min);
    (start, start + min)
}

// ---------------------------------------------------------------------
// Text: MinHash
// ---------------------------------------------------------------------

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn fnv(words: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for w in words {
        for b in w.bytes().chain(std::iter::once(b' ')) {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// The words of `text`, folded the way titles are (case, accents, punctuation
/// gone).
pub fn words(text: &str) -> Vec<String> {
    crate::dedupe::plain(&crate::dedupe::fold(text)).split_whitespace().map(str::to_string).collect()
}

/// The MinHash of the word 3-shingles of `words`. Empty when there are fewer
/// than three words.
pub fn minhash(words: &[String]) -> Vec<u16> {
    if words.len() < 3 {
        return Vec::new();
    }
    let mut slots = vec![u64::MAX; MINHASH_LEN];
    for window in words.windows(3) {
        let h = fnv(&[window[0].as_str(), window[1].as_str(), window[2].as_str()]);
        for (i, slot) in slots.iter_mut().enumerate() {
            let v = splitmix(h ^ splitmix(i as u64 + 1));
            if v < *slot {
                *slot = v;
            }
        }
    }
    slots.into_iter().map(|v| (v >> 48) as u16).collect()
}

/// Estimated Jaccard similarity of two shingle sets from their MinHashes.
/// `None` if either is empty or they differ in length.
pub fn jaccard(a: &[u16], b: &[u16]) -> Option<f32> {
    if a.is_empty() || a.len() != b.len() {
        return None;
    }
    Some(a.iter().zip(b).filter(|(x, y)| x == y).count() as f32 / a.len() as f32)
}

// ---------------------------------------------------------------------
// The fingerprint
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct PdfFingerprint {
    pub page_count: u32,
    pub pages: Vec<PageHash>,
    /// Letters and digits over the pages whose text was read.
    pub text_chars: u32,
    pub has_text: bool,
    /// Empty if there is no text (or under three words).
    pub minhash: Vec<u16>,
}

impl PdfFingerprint {
    /// Sampled pages that carry a hash.
    pub fn usable_pages(&self) -> usize {
        self.pages.iter().filter(|p| !p.blank).count()
    }

    /// `index (4) | hash (8) | blank (1)` a page.
    pub fn encode_pages(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pages.len() * 13);
        for p in &self.pages {
            out.extend_from_slice(&p.index.to_le_bytes());
            out.extend_from_slice(&p.hash.to_le_bytes());
            out.push(p.blank as u8);
        }
        out
    }

    pub fn decode_pages(bytes: &[u8]) -> Option<Vec<PageHash>> {
        if bytes.len() % 13 != 0 {
            return None;
        }
        Some(
            bytes
                .chunks_exact(13)
                .map(|c| PageHash { index: u32::from_le_bytes(c[0..4].try_into().unwrap()), hash: u64::from_le_bytes(c[4..12].try_into().unwrap()), blank: c[12] != 0 })
                .collect(),
        )
    }

    pub fn encode_minhash(&self) -> Vec<u8> {
        self.minhash.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    pub fn decode_minhash(bytes: &[u8]) -> Vec<u16> {
        bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
    }
}

/// The pages (0-based) to hash for a document of `n` pages: every page of a
/// short one, otherwise first three, last three and two in the middle.
pub fn sample_indices(n: usize) -> Vec<usize> {
    if n <= SAMPLE_PAGES {
        return (0..n).collect();
    }
    let mut v = vec![0, 1, 2, n - 3, n - 2, n - 1, n / 3, 2 * n / 3];
    v.sort_unstable();
    v.dedup();
    v
}

/// The pages whose text is read: the first [`TEXT_PAGES`].
pub fn text_page_indices(n: usize) -> Vec<usize> {
    (0..n.min(TEXT_PAGES)).collect()
}

/// Builds a fingerprint from what was read of a PDF. Pure: the PDFium part is
/// [`fingerprint_file`].
pub fn fingerprint_sampled(sampled: &calibre_ebooks::pdf::rasterize::SampledPdf) -> Result<PdfFingerprint> {
    if sampled.page_count == 0 {
        bail!("the document has no pages");
    }
    if sampled.pages.is_empty() {
        bail!("none of the sampled pages could be rendered");
    }
    let pages: Vec<PageHash> = sampled
        .pages
        .iter()
        .map(|p| match page_hash(&p.gray, p.width, p.height) {
            Some(hash) => PageHash { index: p.index as u32, hash, blank: false },
            None => PageHash { index: p.index as u32, hash: 0, blank: true },
        })
        .collect();

    let all_text = sampled.text.join(" ");
    let text_chars = all_text.chars().filter(|c| c.is_alphanumeric()).count() as u32;
    let pages_read = sampled.text.len().max(1) as u32;
    let has_text = text_chars >= MIN_TEXT_CHARS_PER_PAGE * pages_read;
    let minhash = if has_text { minhash(&words(&all_text)) } else { Vec::new() };
    Ok(PdfFingerprint { page_count: sampled.page_count as u32, pages, text_chars, has_text, minhash })
}

/// Fingerprints the PDF at `path`, reading it once through PDFium's file
/// callback. A document PDFium cannot open (encrypted, corrupt) is `Err`; a
/// missing PDFium is an `Err` whose root is
/// [`calibre_ebooks::pdf::rasterize::RasterizeError::Unavailable`] (see
/// [`is_unavailable`]).
pub fn fingerprint_file(path: &Path) -> Result<PdfFingerprint> {
    let sampled = calibre_ebooks::pdf::rasterize::sample_pdf_from_path(path, RENDER_LONG_SIDE, |n| (sample_indices(n), text_page_indices(n)))?;
    fingerprint_sampled(&sampled)
}

/// Whether an error from [`fingerprint_file`] means "no PDFium", which is
/// not the PDF's fault and must not be recorded against it.
pub fn is_unavailable(error: &anyhow::Error) -> bool {
    matches!(error.downcast_ref::<calibre_ebooks::pdf::rasterize::RasterizeError>(), Some(calibre_ebooks::pdf::rasterize::RasterizeError::Unavailable { .. }))
}

// ---------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------

/// How two fingerprints' sampled pages line up.
#[derive(Debug, Clone, PartialEq)]
pub struct VisualMatch {
    pub compared: u32,
    pub matched: u32,
    pub mean_distance: f32,
    /// One of the two has at most two pages.
    pub tiny: bool,
}

/// Compares the sampled pages of `a` and `b`. `None` when they do not match
/// visually, by the rules in the module docs.
pub fn visual_match(a: &PdfFingerprint, b: &PdfFingerprint) -> Option<VisualMatch> {
    let diff = a.page_count as i64 - b.page_count as i64;
    if diff.unsigned_abs() as u32 > MAX_PAGE_DIFF {
        return None;
    }
    let tiny = a.page_count <= 2 || b.page_count <= 2;
    if tiny && a.page_count != b.page_count {
        return None;
    }

    let end = |p: &PageHash, n: u32| n as i64 - 1 - p.index as i64;
    let mut best: Option<VisualMatch> = None;
    // Smallest shifts first, so a tie keeps the plainest alignment.
    for front_shift in [0i64, -1, 1, -2, 2] {
        if tiny && front_shift != 0 {
            continue;
        }
        // a's page i is b's page i - front_shift; at the end the shift is
        // front_shift less the page-count difference.
        let end_shift = front_shift - diff;
        if end_shift.unsigned_abs() as u32 > MAX_PAGE_DIFF {
            continue;
        }
        let mut seen: Vec<(u32, u32)> = Vec::new();
        let mut compared = 0u32;
        let mut matched = 0u32;
        let mut distance_sum = 0u32;
        for pa in a.pages.iter().filter(|p| !p.blank) {
            for pb in b.pages.iter().filter(|p| !p.blank) {
                let front = pa.index as i64 - pb.index as i64 == front_shift;
                let back = end(pa, a.page_count) - end(pb, b.page_count) == end_shift;
                if !(front || back) || seen.contains(&(pa.index, pb.index)) {
                    continue;
                }
                seen.push((pa.index, pb.index));
                compared += 1;
                let d = hamming(pa.hash, pb.hash);
                if d <= PAGE_MATCH_DISTANCE {
                    matched += 1;
                    distance_sum += d;
                }
            }
        }
        let candidate = VisualMatch { compared, matched, mean_distance: if matched > 0 { distance_sum as f32 / matched as f32 } else { 0.0 }, tiny };
        if best.as_ref().is_none_or(|cur| candidate.matched > cur.matched) {
            best = Some(candidate);
        }
    }

    let m = best?;
    let ok = if tiny {
        // Every page of a tiny document, and none of them blank.
        m.compared == a.page_count && m.matched == m.compared
    } else {
        m.compared >= MIN_COMPARED && m.matched as f32 >= MATCH_FRACTION * m.compared as f32
    };
    ok.then_some(m)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PdfKind {
    /// Same pages, one with a text layer and one without (or two different
    /// OCR runs).
    OcrVariant,
    /// Same pages, same text (or both image-only): an optimizer's output.
    Resave,
    /// Same words, different pages.
    TextMatch,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PdfMatch {
    pub kind: PdfKind,
    pub confidence: f32,
    pub detail: String,
}

/// What a pair of fingerprints are, if anything.
pub fn classify(a: &PdfFingerprint, b: &PdfFingerprint) -> Option<PdfMatch> {
    let jac = jaccard(&a.minhash, &b.minhash);
    if let Some(v) = visual_match(a, b) {
        let pages = format!("{} of {} sampled pages match (page counts {} and {})", v.matched, v.compared, a.page_count, b.page_count);
        let penalty = if v.tiny { TINY_PENALTY } else { 1.0 };
        let (kind, confidence, what) = match (a.has_text, b.has_text) {
            (true, false) | (false, true) => (PdfKind::OcrVariant, CONFIDENCE_OCR, "one has a text layer and the other is image-only".to_string()),
            (false, false) => (PdfKind::Resave, CONFIDENCE_RESAVE, "both are image-only".to_string()),
            (true, true) => match jac {
                Some(j) if j >= SAME_TEXT_JACCARD => (PdfKind::Resave, CONFIDENCE_RESAVE, format!("same text (similarity {j:.2})")),
                Some(j) if j >= DIFFERENT_TEXT_JACCARD => (PdfKind::OcrVariant, CONFIDENCE_OCR_TWO_LAYERS, format!("different text layers (similarity {j:.2})")),
                // The same look, other words: a template, not a copy.
                _ => return None,
            },
        };
        return Some(PdfMatch { kind, confidence: confidence * penalty, detail: format!("{pages}; {what}") });
    }
    let j = jac?;
    let close = (a.page_count as i64 - b.page_count as i64).unsigned_abs() as u32 <= MAX_PAGE_DIFF;
    (a.has_text && b.has_text && close && j >= TEXT_MATCH_JACCARD && a.text_chars >= TEXT_MATCH_MIN_CHARS && b.text_chars >= TEXT_MATCH_MIN_CHARS)
        .then(|| PdfMatch { kind: PdfKind::TextMatch, confidence: CONFIDENCE_TEXT, detail: format!("same text (similarity {j:.2}) on different pages ({} and {} pages)", a.page_count, b.page_count) })
}

// ---------------------------------------------------------------------
// The candidate index
// ---------------------------------------------------------------------

const BANDS: usize = 16;
const ROWS: usize = MINHASH_LEN / BANDS;

/// What [`find_pairs`] did, for the scale tests and the logs.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IndexStats {
    pub fingerprints: usize,
    /// Pairs the index proposed.
    pub candidates: usize,
    /// Pairs that were verified as a match.
    pub matches: usize,
    /// Posting lists skipped for being longer than [`MAX_POSTING`].
    pub skipped_postings: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PdfPair {
    /// Indexes into the slice given to [`find_pairs`]; `a < b`.
    pub a: usize,
    pub b: usize,
    pub found: PdfMatch,
}

fn band_key(minhash: &[u16], band: usize) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ band as u64;
    for v in &minhash[band * ROWS..(band + 1) * ROWS] {
        h = splitmix(h ^ *v as u64);
    }
    h
}

/// Every matching pair among `fps`, by the index and then verification.
/// Work grows with the number of *candidate* pairs, not with `n^2`.
pub fn find_pairs(fps: &[PdfFingerprint]) -> (Vec<PdfPair>, IndexStats) {
    let mut stats = IndexStats { fingerprints: fps.len(), ..Default::default() };

    let mut visual: HashMap<(u32, u8, u16), Vec<u32>> = HashMap::new();
    let mut text: HashMap<(u32, u8, u64), Vec<u32>> = HashMap::new();
    for (i, fp) in fps.iter().enumerate() {
        for p in fp.pages.iter().filter(|p| !p.blank) {
            for c in 0..4u8 {
                let chunk = (p.hash >> (16 * c as u32)) as u16;
                let list = visual.entry((fp.page_count, c, chunk)).or_default();
                if list.last() != Some(&(i as u32)) {
                    list.push(i as u32);
                }
            }
        }
        if fp.minhash.len() == MINHASH_LEN {
            for band in 0..BANDS {
                text.entry((fp.page_count, band as u8, band_key(&fp.minhash, band))).or_default().push(i as u32);
            }
        }
    }
    stats.skipped_postings = visual.values().filter(|l| l.len() > MAX_POSTING).count() + text.values().filter(|l| l.len() > MAX_POSTING).count();

    let mut pairs = Vec::new();
    for (i, fp) in fps.iter().enumerate() {
        let mut hits: HashMap<u32, u32> = HashMap::new();
        let usable: Vec<&PageHash> = fp.pages.iter().filter(|p| !p.blank).collect();
        for p in usable.iter() {
            let mut slot_hits: Vec<u32> = Vec::new();
            for pc in fp.page_count.saturating_sub(MAX_PAGE_DIFF)..=fp.page_count + MAX_PAGE_DIFF {
                for c in 0..4u8 {
                    let chunk = (p.hash >> (16 * c as u32)) as u16;
                    if let Some(list) = visual.get(&(pc, c, chunk)) {
                        if list.len() > MAX_POSTING {
                            continue;
                        }
                        slot_hits.extend(list.iter().copied().filter(|&j| j as usize > i));
                    }
                }
            }
            slot_hits.sort_unstable();
            slot_hits.dedup();
            for j in slot_hits {
                *hits.entry(j).or_insert(0) += 1;
            }
        }
        let needed = MIN_CANDIDATE_PAGES.min(usable.len() as u32).max(1);
        let mut candidates: Vec<u32> = hits.into_iter().filter(|&(_, n)| n >= needed).map(|(j, _)| j).collect();

        if fp.minhash.len() == MINHASH_LEN {
            for pc in fp.page_count.saturating_sub(MAX_PAGE_DIFF)..=fp.page_count + MAX_PAGE_DIFF {
                for band in 0..BANDS {
                    if let Some(list) = text.get(&(pc, band as u8, band_key(&fp.minhash, band))) {
                        if list.len() > MAX_POSTING {
                            continue;
                        }
                        candidates.extend(list.iter().copied().filter(|&j| j as usize > i));
                    }
                }
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        stats.candidates += candidates.len();
        for j in candidates {
            if let Some(found) = classify(fp, &fps[j as usize]) {
                stats.matches += 1;
                pairs.push(PdfPair { a: i, b: j as usize, found });
            }
        }
    }
    (pairs, stats)
}

// ---------------------------------------------------------------------
// The fingerprint job
// ---------------------------------------------------------------------

/// Most files read at once. PDFium serialises its calls (`thread_safe`), so
/// extra workers overlap only the hashing and the disk wait; two is what that
/// can use. (Not measured against a real PDFium in the tranche-9 sandbox: see
/// the PR notes.)
pub const MAX_FINGERPRINT_WORKERS: usize = 2;

/// What the fingerprint job did.
#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct FingerprintReport {
    /// Distinct file contents fingerprinted.
    pub fingerprinted: usize,
    /// PDF formats that needed one when the job started.
    pub total: usize,
    /// `(path, error)` for PDFs PDFium could not read. Recorded, so they are
    /// not retried.
    pub failed: Vec<(String, String)>,
    pub failed_count: usize,
    /// Rows whose file is not on disk. Not recorded: a restored file is retried.
    pub missing: Vec<String>,
    pub missing_count: usize,
    pub cancelled: bool,
}

/// Fingerprints every PDF format that has a content hash and no current
/// fingerprint. Resumable by construction, like the hash backfill: the work
/// list is "no current fingerprint", so each row written shortens it. A
/// failed PDF is recorded as failed (with the reason) and is not retried
/// until [`FINGERPRINT_VERSION`] changes.
///
/// Errors (rather than recording failures) if PDFium is not installed.
pub fn fingerprint_pdfs(cache: &Cache, options: &PipelineOptions, control: &JobControl) -> Result<FingerprintReport> {
    use std::sync::atomic::Ordering;
    control.set_phase("fingerprinting");
    let library = cache.backend.library_path.clone();
    let todo = cache.checksums().pdf_fingerprints_pending()?;
    control.total.store(todo.len() as u64, Ordering::Relaxed);
    let mut report = FingerprintReport { total: todo.len(), ..Default::default() };
    if todo.is_empty() {
        return Ok(report);
    }
    if !calibre_ebooks::pdf::rasterize::is_available() {
        bail!("PDF fingerprinting needs the PDFium library, which was not found (run `cargo xtask fetch-pdfium` or set {})", calibre_ebooks::pdf::rasterize::LIBRARY_PATH_VAR);
    }

    // One file read per distinct content: books sharing a hash share a row.
    let mut by_hash: Vec<(String, Vec<(String, String, String)>)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for p in &todo {
        let slot = *at.entry(p.hash.clone()).or_insert_with(|| {
            by_hash.push((p.hash.clone(), Vec::new()));
            by_hash.len() - 1
        });
        by_hash[slot].1.push((p.folder.clone(), p.name.clone(), p.format.clone()));
    }
    // Formats sharing a hash after the first are done with it.
    let shared: u64 = (todo.len() - by_hash.len()) as u64;

    let workers = options.workers.clamp(1, MAX_FINGERPRINT_WORKERS);
    let mut abort: Option<anyhow::Error> = None;
    for chunk in by_hash.chunks(options.batch_size.max(1)) {
        if control.is_cancelled() {
            break;
        }
        let results = parallel_map(chunk, workers, control, |(_, locations)| -> Result<Option<(String, Result<PdfFingerprint>)>> {
            for (folder, name, format) in locations {
                let Some(path) = format_file_path(&library, folder, name, format) else { continue };
                let display = path.display().to_string();
                return match fingerprint_file(&path) {
                    Ok(fp) => Ok(Some((display, Ok(fp)))),
                    Err(e) if is_unavailable(&e) => Err(e),
                    Err(e) => Ok(Some((display, Err(e)))),
                };
            }
            Ok(None)
        });
        for ((hash, locations), result) in chunk.iter().zip(results) {
            let Some(result) = result else { continue };
            match result {
                Err(e) => {
                    abort = Some(e);
                    break;
                }
                Ok(None) => {
                    control.errors.fetch_add(1, Ordering::Relaxed);
                    report.missing_count += 1;
                    if report.missing.len() < 100 {
                        let (folder, name, format) = &locations[0];
                        report.missing.push(format!("{folder}/{name}.{}", format.to_ascii_lowercase()));
                    }
                }
                Ok(Some((path, Ok(fp)))) => {
                    cache.checksums().record_pdf_fingerprint(hash, &Ok(fp))?;
                    control.fingerprinted.fetch_add(1, Ordering::Relaxed);
                    report.fingerprinted += 1;
                    let _ = path;
                }
                Ok(Some((path, Err(e)))) => {
                    let why = format!("{e:#}");
                    cache.checksums().record_pdf_fingerprint(hash, &Err(why.clone()))?;
                    control.errors.fetch_add(1, Ordering::Relaxed);
                    report.failed_count += 1;
                    if report.failed.len() < 100 {
                        report.failed.push((path, why));
                    }
                }
            }
        }
        if let Some(e) = abort.take() {
            return Err(anyhow!("{e:#}"));
        }
        options.batch_done(control);
    }
    control.already_known.fetch_add(shared, Ordering::Relaxed);
    report.cancelled = control.is_cancelled();
    Ok(report)
}

// ---------------------------------------------------------------------
// Reading a library
// ---------------------------------------------------------------------

/// Where the library's PDFs stand, for the scan response and the UI.
#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct PdfStatus {
    /// PDF formats with a current fingerprint.
    pub fingerprinted: usize,
    /// PDF formats that could not be fingerprinted (recorded, not retried).
    pub failed: usize,
    /// PDF formats with a content hash and no current fingerprint: what the
    /// fingerprint job would do.
    pub pending: usize,
    /// PDF formats with no content hash yet; the hash backfill gives them one.
    pub unhashed: usize,
}

/// A fingerprint and the books that hold that content.
pub struct LibraryFingerprint {
    pub hash: String,
    pub books: Vec<i32>,
    pub fingerprint: PdfFingerprint,
    pub size: i64,
}

/// One edge to hand to the grouping.
#[derive(Debug, Clone, PartialEq)]
pub struct PdfEdge {
    pub a: i32,
    pub b: i32,
    pub found: PdfMatch,
}

/// Matches every fingerprinted PDF in the library against the others.
pub fn library_edges(cache: &Cache) -> Result<(Vec<PdfEdge>, IndexStats)> {
    Ok(edges_for(&cache.checksums().pdf_fingerprints()?))
}

/// [`library_edges`] over fingerprints already loaded. One edge a pair of
/// *contents*, between the first book of each: books sharing a content hash
/// are already joined by the content tier.
pub fn edges_for(loaded: &[LibraryFingerprint]) -> (Vec<PdfEdge>, IndexStats) {
    let loaded: Vec<&LibraryFingerprint> = loaded.iter().filter(|l| !l.books.is_empty()).collect();
    let fps: Vec<PdfFingerprint> = loaded.iter().map(|l| l.fingerprint.clone()).collect();
    let (pairs, stats) = find_pairs(&fps);
    let edges = pairs.into_iter().map(|p| PdfEdge { a: loaded[p.a].books[0], b: loaded[p.b].books[0], found: p.found }).collect();
    (edges, stats)
}

#[cfg(test)]
mod tests;
