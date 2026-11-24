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
//! module is Lexmechanic-specific.
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
//! * a 256-bit page signature ([`page_signature`]) of up to [`SAMPLE_PAGES`] = 8
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
//! ## The page signature
//!
//! The page is rendered by PDFium to a grayscale image with a long side of
//! [`RENDER_LONG_SIDE`] = 640 px (`rasterize::sample_pdf_from_path`), then
//! [`page_signature`] makes it comparable across the differences between a
//! scan and its copy:
//!
//! 1. **ink** ([`page_grids`]): a pixel is ink when it is darker than the mean
//!    of its own neighbourhood by 8% of the paper level, so paper tone,
//!    uneven lighting and the strength of JPEG blur do not move it;
//! 2. **trim**: a row or column with one unbroken run of ink across more than
//!    55% of it (gaps of two pixels allowed, for noise) is a scanner's border
//!    and is dropped. A line of text is not, however dense: it breaks at every
//!    word gap. The page is then cropped to its ink's 1st-99th percentile, so
//!    a shifted, bordered or differently framed scan crops to the same text
//!    block;
//! 3. **signature**: the *gray* page (not the ink map, which only decides
//!    the crop and blankness) is averaged over the crop in a 17 x 16 grid, and
//!    each cell against its right-hand neighbour is one bit (a difference
//!    hash, 16 x 16 = 256 bits). A pair of cells within half a gray level of
//!    each other gives an *untrusted* bit (it is a coin flip between two
//!    encodings), recorded in a mask; [`page_distance`] counts disagreements
//!    among the bits both pages trust, scaled to 256, and refuses to compare
//!    pages sharing fewer than [`MIN_TRUSTED_BITS`].
//!
//! Why not a DCT pHash: the pages of one book are the same gray texture (one
//! font, one line grid), and a low-frequency transform sees only the shape of
//! the text block, which every page shares. What tells pages apart is the
//! content of each cell: where a paragraph ends, a heading, which words fall
//! where. The first version (a 64-bit pHash at 256 px) matched a re-encoded
//! scan to its own page only 55-75% of the time and to some *other* page
//! 13-32% of the time.
//!
//! A page whose ink is under 2% of its pixels is **blank** (no signature,
//! never compared): a white page, a uniformly dark one, paper grain, and a
//! page holding only a page number all land here.
//!
//! # Matching
//!
//! Never all-pairs ([`find_pairs`]).
//!
//! **Candidates** come from bit-sampling LSH on the page signatures: 16
//! tables, each keyed by a fixed choice of 14 of the 256 bits (a page is left
//! out of a table where any of its 14 is untrusted). Documents are taken a
//! page count at a time with the two counts above it, and two pages collide
//! when they share a key *and* their page numbers are within
//! [`MAX_PAGE_DIFF`] (a copy collides on its own page, give or take a shifted
//! cover; anything else is chance). A pair is a candidate when at least
//! [`MIN_CANDIDATE_PAGES`] = 2 of its sampled pages collide (one for a
//! document with at most two usable pages). A key shared by more than
//! [`MAX_POSTING`] pages says nothing and is skipped. A copy is found if it
//! collides in any table: at the measured same-page distances (under 20 of
//! 256 bits, 8%) a table hits with probability about 0.31 and some table of 16
//! with about 99.7%, a page at a time. Random pages collide about once in
//! 1,000 tables, and needing two aligned pages makes a random candidate pair
//! rare (3,000 random fingerprints of near-equal length give under 3,000
//! candidates of 4.5 million pairs). Verification, not the index, decides.
//!
//! **Verify** ([`visual_match`]) aligns the sample pages. Equal page counts
//! compare the same positions. Unequal counts (a cover added, a trailing
//! blank dropped, up to [`MAX_PAGE_DIFF`] = 2) try every shift of the front
//! frame, with the end frame shifted by the page-count difference less that,
//! and keep the alignment with the most matching pages. A pair of pages
//! match at a [`page_distance`] of at most [`PAGE_MATCH_DISTANCE`] = 48 of
//! 256. The evidence is the ignored benchmark
//! `tests/pdf_signature_benchmark.rs`, on real PDFium renders of two
//! generated 24-page Times-Roman books:
//!
//! | Case | Same page | Other pages |
//! | --- | --- | --- |
//! | clean render, other pages of the book / the other book | | min 89 / 95, median 120 |
//! | independent scan (noise, border, tone, JPEG q75), no skew | max 14, p95 10 | min 92 |
//! | one scan re-encoded at q85 / q50 | max 1 | min 61 |
//! | one scan downsampled x0.5 at q30 / x0.35 at q20 | max 17 / 15 | min 66 / 60 |
//! | independent scan skewed 0.4 or 0.8 degrees | 107-154 | 88-162 |
//!
//! Every page got a signature in every case. The last row is a limit: two
//! *independent* scans of one book on different scanners, with different
//! skew, do not match by pixels (their OCR text can, as `pdf_text_match`).
//! What this finds is one scan and the copies made from it: OCR'd, optimised,
//! re-saved. The unit test `same_pages_are_close_and_different_pages_are_far`
//! checks the same on synthetic pages (400 shifted, bordered, JPEG and
//! downsampled copies of 80: same-page max 41, other pages min 90). A pair of
//! documents matches visually when at least [`MIN_COMPARED`] = 3 non-blank
//! pages were compared and at least 75% of them match. A document of one or
//! two pages needs equal page counts and *every* page to match, and gets a
//! confidence penalty.
//!
//! **Classify** ([`classify`]), given a visual match:
//!
//! | Evidence | Kind | Confidence band |
//! | --- | --- | --- |
//! | exactly one side has a text layer | `pdf_ocr_variant` | 0.72-0.92 |
//! | neither has text | `pdf_resave` | 0.80-0.97 |
//! | both have text, MinHash Jaccard >= 0.8 | `pdf_resave` | 0.80-0.97 |
//! | both have text, **both scanned**, 0.5 <= *distinctive* Jaccard < 0.8, over at least 64 distinctive slots | `pdf_ocr_variant` (two OCR runs) | 0.68-0.88 |
//! | both have text, anything lower | *no match*: the same look but other words is a different book (a template) | |
//!
//! # Graded confidence (tranche 16)
//!
//! The tier says *what kind* of copy a pair is; where the confidence falls in
//! the tier's band says *how well the measurements agree*. [`grade`] takes what
//! was measured ([`Measured`]) and returns `low + strength * (high - low)`, then
//! the tiny-document penalty. `strength` is 0 to 1:
//!
//! * **pages**: the fraction of compared sample pages that matched, from
//!   [`MATCH_FRACTION`] (0) to all of them (1);
//! * **distance**: the mean page distance of the matched pages, from
//!   [`PAGE_MATCH_DISTANCE`] (0) to identical (1);
//! * **text**: the text Jaccard, over the range that tier accepts (0.8-1.0 for
//!   a re-save, 0.5-0.8 for two OCR layers, 0.9-1.0 for a text match).
//!
//! A re-save with text and two OCR layers weigh them 0.4 / 0.3 / 0.3; a pair
//! with no text to compare (a scan and its OCR, two image-only copies) weighs
//! pages and distance equally; a text match (no visual match at all) weighs the
//! text 0.7 and how close the page counts are 0.3. A stronger measurement never
//! lowers the score (tested), and the bands do not overlap their neighbours'
//! tops by more than they must, so sorting by confidence still puts the tiers
//! in a sensible order: `pdf_text_match` 0.50-0.75 tops out below a re-save's
//! floor. The measured numbers travel with the match ([`PdfMatch::measured`]) and
//! are written into its `detail`.
//!
//! **Scanned** (fingerprint version 3): at least 75% of the sampled pages have
//! an image object covering 80% or more of the page. Two OCR runs of one scan
//! are text laid over page images; the real trial still produced nine false
//! `pdf_ocr_variant`s from the both-text sub-tier, every one a short born-digital
//! document on a shared layout (x86 instruction reference pages `MOVHPS`/`MOVLPS`,
//! `CLC`/`CLD`; `Booster Set Guide - Cyberpunk`/`- Dystopian`; `AWN`/`CWN -
//! Character Sheet`), whose pages are vector text and no images. A pair where
//! either side is not scanned is not an OCR variant of each other.
//!
//! "Distinctive" is the point of tranche 11. Two OCR runs of one scan share
//! almost every content word (a recognition error costs only the three
//! shingles around it), so their Jaccard is high (about 0.75 at a 3% error rate
//! per run). Born-digital documents on one *template* (the Intel instruction
//! reference pages `MOVHPS`/`MOVLPS`, `CMPSD`/`CMPSS`; a `Names - Female` / `-
//! Male` pair; a publisher's whole line of supplements) share the template's
//! boilerplate and have a moderate Jaccard, which the old lower bound of 0.3
//! accepted: a 49-book group of unrelated supplements formed this way. The
//! lower bound is now 0.5, and the Jaccard is taken only over the MinHash slots
//! that are not **library-wide boilerplate** ([`StopSlots`]): a slot value held
//! by at least `max(5, 0.5%)` of the library's documents is a shingle that
//! many documents share, and says nothing about these two. A pair left with
//! fewer than [`MIN_DISTINCTIVE_SLOTS`] = 64 of the 128 slots has too little
//! distinctive text to be judged at all. (Whether the pages are scanned images
//! is *not* used: it would need the fingerprint to record it and a version
//! bump, and a born-digital re-save with the same words is a legitimate
//! `pdf_resave`.)
//!
//! With no visual match, two text-bearing PDFs of at least
//! [`TEXT_MATCH_MIN_CHARS`] letters whose *distinctive* Jaccard is >= 0.9 over at
//! least 64 distinctive slots and whose page counts are within two are
//! `pdf_text_match` (0.60): the same words re-typeset. These come from a second
//! index, MinHash LSH (16 bands of 8). Old Google Books scans have no OCR text
//! in their first 48 pages except Google's standard notice ("This is a digital
//! copy of a book that was preserved for generations on library shelves..."),
//! so every such book has the same ~3,000 characters and a Jaccard of 1.00 with
//! every other. The notice is in hundreds of documents, so its slots are
//! boilerplate, are dropped, and what is left (nothing) is not enough to match.
//!
//! # Groups are not chains
//!
//! Pairs become groups in [`crate::dedupe`], which used to union every pair
//! transitively, so a chain of weak pairs `A~B`, `B~C`, `C~D`... made one big
//! group. Page-content pairs now join a group by **complete linkage**: two
//! clusters of books (books holding identical files count as one) merge only if
//! *every* cross pair has a page-content edge. A cluster of up to three is
//! allowed to chain (a scan, its OCR and an optimized copy often have two
//! edges and a missing third). Metadata evidence is unchanged.
//!
//! # What can go wrong
//!
//! * **Pages that look alike.** Different pages of one book come no closer
//!   than about 90 of 256 bits on clean renders, but a scan's pages are
//!   closer to each other (60 at worst above) than clean ones, and two books
//!   from one publisher template are untested. The guards are the 75%
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
use serde::{Deserialize, Serialize};

use crate::cache::Cache;
use crate::scan_pipeline::{format_file_path, parallel_map, JobControl, PipelineOptions};

mod ink;
pub use ink::{ink_grid, page_grids, PageGrids};

/// Bump when the hash, sampling or MinHash changes: every row with another
/// version is recomputed.
pub const FINGERPRINT_VERSION: u32 = 3;
/// Pages hashed per PDF.
pub const SAMPLE_PAGES: usize = 8;
/// Long side of the render the hash is made from.
pub const RENDER_LONG_SIDE: u32 = 640;
/// Pages (from the start) whose text is read.
pub const TEXT_PAGES: usize = 48;
/// A text layer is at least this many letters or digits a page, on average.
pub const MIN_TEXT_CHARS_PER_PAGE: u32 = 20;
/// Slots in a text MinHash.
pub const MINHASH_LEN: usize = 128;
/// [`page_distance`], in bits of 256, at or under which two pages are the same.
pub const PAGE_MATCH_DISTANCE: u32 = 48;
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
/// Trusted bits two pages must share to be compared at all.
pub const MIN_TRUSTED_BITS: u32 = 64;
/// Bits sampled for one table of the page index, and the number of tables.
const TABLE_BITS: usize = 14;
const TABLES: usize = 16;
/// Jaccard at or above which two text layers are the same text.
pub const SAME_TEXT_JACCARD: f32 = 0.8;
/// Jaccard (over distinctive slots) below which two text layers are
/// different books. Two OCR runs of one scan share most of their words (an
/// error in a run costs the three shingles that contain it, so a 3% word
/// error rate in each run leaves a Jaccard near 0.75); two documents built
/// from one template (instruction-set reference pages, a publisher's series)
/// share the template and little else.
pub const DIFFERENT_TEXT_JACCARD: f32 = 0.5;
/// A shingle (MinHash slot value) held by at least this many distinct
/// documents is boilerplate, whatever the fraction; see [`StopSlots`].
pub const STOP_MIN_DOCS: usize = 5;
/// ... or by this fraction of the library's documents with text, if more.
pub const STOP_FRACTION: f32 = 0.005;
/// Text slots left after dropping boilerplate that a text comparison needs
/// to count at all (of [`MINHASH_LEN`]).
pub const MIN_DISTINCTIVE_SLOTS: usize = 64;
/// Jaccard for a text-only match.
pub const TEXT_MATCH_JACCARD: f32 = 0.9;
/// Letters a text-only match needs on each side.
pub const TEXT_MATCH_MIN_CHARS: u32 = 2000;

/// The confidence band of each tier, `(floor, ceiling)`: a match is placed in
/// its tier's band by [`grade`]. Tiers keep their order: a re-save is never
/// less certain than the floor of its band, and a text match never more than
/// its ceiling.
pub const BAND_RESAVE: (f32, f32) = (0.80, 0.97);
pub const BAND_OCR: (f32, f32) = (0.72, 0.92);
pub const BAND_OCR_TWO_LAYERS: (f32, f32) = (0.68, 0.88);
pub const BAND_TEXT: (f32, f32) = (0.50, 0.75);
/// Multiplier for a document of one or two pages.
pub const TINY_PENALTY: f32 = 0.85;

// ---------------------------------------------------------------------
// The page hash
// ---------------------------------------------------------------------

/// One sampled page: a 256-bit signature, and which of its bits are to be
/// trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageHash {
    /// 0-based page number.
    pub index: u32,
    /// The signature bits; zero when `blank`.
    pub bits: [u64; 4],
    /// The bits whose comparison was decisive when the page was read. A bit
    /// that is not set is a coin flip between two encodings of one scan, so
    /// [`page_distance`] ignores it.
    pub mask: [u64; 4],
    pub blank: bool,
}

impl PageHash {
    /// A page whose every bit is trusted.
    pub fn new(index: u32, bits: [u64; 4]) -> Self {
        PageHash { index, bits, mask: [u64::MAX; 4], blank: false }
    }

    pub fn blank(index: u32) -> Self {
        PageHash { index, bits: [0; 4], mask: [0; 4], blank: true }
    }
}

/// Columns and rows of the grid of cell means a page is reduced to. One bit a
/// pair of neighbouring cells in a row: 16 x 16 = 256 bits.
const GRID_COLS: usize = 17;
const GRID_ROWS: usize = 16;
/// Bits in a signature.
pub const SIGNATURE_BITS: u32 = 256;
/// A cell that differs from its right-hand neighbour by no more than this many
/// gray levels (of 255) gives an *untrusted* bit.
const BIT_MARGIN: f32 = 0.5;

/// The signature of one grayscale page (one byte a pixel, row-major) as
/// `(bits, trusted)`; `None` for a blank page. See the module docs.
pub fn page_signature(gray: &[u8], width: u32, height: u32) -> Option<([u64; 4], [u64; 4])> {
    let grids = page_grids(gray, width, height, GRID_COLS, GRID_ROWS)?;
    let (mut bits, mut mask) = ([0u64; 4], [0u64; 4]);
    for y in 0..GRID_ROWS {
        for x in 0..GRID_COLS - 1 {
            let diff = grids.mean[y * GRID_COLS + x + 1] - grids.mean[y * GRID_COLS + x];
            let bit = y * (GRID_COLS - 1) + x;
            if diff.abs() > BIT_MARGIN {
                mask[bit / 64] |= 1 << (bit % 64);
                if diff > 0.0 {
                    bits[bit / 64] |= 1 << (bit % 64);
                }
            }
        }
    }
    Some((bits, mask))
}

/// How far apart two pages are, in bits out of [`SIGNATURE_BITS`]: the
/// disagreements among the bits both trust, scaled up to the full width. `None`
/// when they trust fewer than [`MIN_TRUSTED_BITS`] in common (a nearly empty
/// page says too little to compare).
pub fn page_distance(a: &PageHash, b: &PageHash) -> Option<u32> {
    let (mut both, mut bad) = (0u32, 0u32);
    for i in 0..4 {
        let common = a.mask[i] & b.mask[i];
        both += common.count_ones();
        bad += ((a.bits[i] ^ b.bits[i]) & common).count_ones();
    }
    (both >= MIN_TRUSTED_BITS).then(|| bad * SIGNATURE_BITS / both)
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

/// The MinHash slots whose value is **boilerplate**: the same value in the
/// same slot in at least `max(`[`STOP_MIN_DOCS`]`, `[`STOP_FRACTION`]` of the
/// documents with text)` distinct documents.
///
/// A slot holds the smallest hash of the document's word 3-shingles, so a
/// value shared by many unrelated documents is a shingle they all contain:
/// Google's "This is a digital copy of a book that was preserved for
/// generations..." notice (the only text of an old Google Books scan with no
/// OCR in its first pages), a licence, a publisher's page furniture. Such a
/// slot is no evidence that two documents are the same text, so
/// [`distinctive_jaccard`] leaves it out. Counted over distinct contents (a
/// copy of a file is one row), so one book in several versions is not mistaken
/// for boilerplate until it has five of them.
#[derive(Debug, Default, Clone)]
pub struct StopSlots {
    stop: std::collections::HashSet<u32>,
}

impl StopSlots {
    fn key(slot: usize, value: u16) -> u32 {
        (slot as u32) << 16 | value as u32
    }

    /// Counts slot values over `fps`.
    pub fn from_fingerprints(fps: &[PdfFingerprint]) -> Self {
        Self::from_refs(fps.iter())
    }

    /// [`Self::from_fingerprints`] over borrowed fingerprints, wherever they live.
    pub fn from_refs<'a>(fps: impl Iterator<Item = &'a PdfFingerprint> + Clone) -> Self {
        let with_text = fps.clone().filter(|f| f.minhash.len() == MINHASH_LEN).count();
        let threshold = STOP_MIN_DOCS.max((with_text as f32 * STOP_FRACTION).ceil() as usize);
        let mut counts: HashMap<u32, usize> = HashMap::new();
        for f in fps.filter(|f| f.minhash.len() == MINHASH_LEN) {
            for (slot, &v) in f.minhash.iter().enumerate() {
                *counts.entry(Self::key(slot, v)).or_insert(0) += 1;
            }
        }
        StopSlots { stop: counts.into_iter().filter(|(_, n)| *n >= threshold).map(|(k, _)| k).collect() }
    }

    pub fn is_stop(&self, slot: usize, value: u16) -> bool {
        !self.stop.is_empty() && self.stop.contains(&Self::key(slot, value))
    }

    pub fn len(&self) -> usize {
        self.stop.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stop.is_empty()
    }
}

/// [`jaccard`] over the slots where neither side holds boilerplate: `(estimate,
/// slots compared)`. `None` if there are no such slots or the hashes cannot be
/// compared. The caller decides how many compared slots are enough
/// ([`MIN_DISTINCTIVE_SLOTS`]).
pub fn distinctive_jaccard(a: &[u16], b: &[u16], stops: &StopSlots) -> Option<(f32, usize)> {
    if a.is_empty() || a.len() != b.len() {
        return None;
    }
    let (mut usable, mut equal) = (0usize, 0usize);
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        if stops.is_stop(i, *x) || stops.is_stop(i, *y) {
            continue;
        }
        usable += 1;
        if x == y {
            equal += 1;
        }
    }
    (usable > 0).then(|| (equal as f32 / usable as f32, usable))
}

// ---------------------------------------------------------------------
// The fingerprint
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PdfFingerprint {
    pub page_count: u32,
    pub pages: Vec<PageHash>,
    /// Letters and digits over the pages whose text was read.
    pub text_chars: u32,
    pub has_text: bool,
    /// The sampled pages are images: at least 75% of them have an image
    /// object covering 80% or more of the page (a scan, with or without an
    /// invisible OCR text layer). A born-digital PDF is not.
    pub scanned: bool,
    /// Empty if there is no text (or under three words).
    pub minhash: Vec<u16>,
}

impl PdfFingerprint {
    /// Sampled pages that carry a hash.
    pub fn usable_pages(&self) -> usize {
        self.pages.iter().filter(|p| !p.blank).count()
    }

    /// `index (4) | blank (1) | bits (32) | trusted mask (32)` a page.
    pub fn encode_pages(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pages.len() * PAGE_RECORD);
        for p in &self.pages {
            out.extend_from_slice(&p.index.to_le_bytes());
            out.push(p.blank as u8);
            for w in p.bits.iter().chain(&p.mask) {
                out.extend_from_slice(&w.to_le_bytes());
            }
        }
        out
    }

    pub fn decode_pages(bytes: &[u8]) -> Option<Vec<PageHash>> {
        if bytes.len() % PAGE_RECORD != 0 {
            return None;
        }
        let word = |c: &[u8], i: usize| u64::from_le_bytes(c[5 + 8 * i..13 + 8 * i].try_into().unwrap());
        Some(
            bytes
                .chunks_exact(PAGE_RECORD)
                .map(|c| PageHash {
                    index: u32::from_le_bytes(c[0..4].try_into().unwrap()),
                    blank: c[4] != 0,
                    bits: [word(c, 0), word(c, 1), word(c, 2), word(c, 3)],
                    mask: [word(c, 4), word(c, 5), word(c, 6), word(c, 7)],
                })
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
pub fn fingerprint_sampled(sampled: &lexmechanic_ebooks::pdf::rasterize::SampledPdf) -> Result<PdfFingerprint> {
    if sampled.page_count == 0 {
        bail!("the document has no pages");
    }
    if sampled.pages.is_empty() {
        bail!("none of the sampled pages could be rendered");
    }
    let pages: Vec<PageHash> = sampled
        .pages
        .iter()
        .map(|p| match page_signature(&p.gray, p.width, p.height) {
            Some((bits, mask)) => PageHash { index: p.index as u32, bits, mask, blank: false },
            None => PageHash::blank(p.index as u32),
        })
        .collect();

    let all_text = sampled.text.join(" ");
    let text_chars = all_text.chars().filter(|c| c.is_alphanumeric()).count() as u32;
    let pages_read = sampled.text.len().max(1) as u32;
    let has_text = text_chars >= MIN_TEXT_CHARS_PER_PAGE * pages_read;
    let minhash = if has_text { minhash(&words(&all_text)) } else { Vec::new() };
    let scanned = !sampled.pages.is_empty() && sampled.pages.iter().filter(|p| p.image_page).count() * 4 >= sampled.pages.len() * 3;
    Ok(PdfFingerprint { page_count: sampled.page_count as u32, pages, text_chars, has_text, scanned, minhash })
}

/// Fingerprints the PDF at `path`, reading it once through PDFium's file
/// callback. A document PDFium cannot open (encrypted, corrupt) is `Err`; a
/// missing PDFium is an `Err` whose root is
/// [`lexmechanic_ebooks::pdf::rasterize::RasterizeError::Unavailable`] (see
/// [`is_unavailable`]).
pub fn fingerprint_file(path: &Path) -> Result<PdfFingerprint> {
    let sampled = lexmechanic_ebooks::pdf::rasterize::sample_pdf_from_path(path, RENDER_LONG_SIDE, |n| (sample_indices(n), text_page_indices(n)))?;
    fingerprint_sampled(&sampled)
}

/// Whether an error from [`fingerprint_file`] means "no PDFium", which is
/// not the PDF's fault and must not be recorded against it.
pub fn is_unavailable(error: &anyhow::Error) -> bool {
    matches!(error.downcast_ref::<lexmechanic_ebooks::pdf::rasterize::RasterizeError>(), Some(lexmechanic_ebooks::pdf::rasterize::RasterizeError::Unavailable { .. })) || error.downcast_ref::<NoWorker>().is_some()
}

/// The PDF worker process could not be started: nothing is the file's fault.
#[derive(Debug)]
struct NoWorker(String);

impl std::fmt::Display for NoWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "could not start the PDF worker: {}", self.0)
    }
}

impl std::error::Error for NoWorker {}

// ---------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------

/// One pair of sampled pages that were compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PagePair {
    /// 0-based page of the first / second document.
    pub a: u32,
    pub b: u32,
    /// [`page_distance`], of 256.
    pub distance: u32,
    pub matched: bool,
}

/// How two fingerprints' sampled pages line up.
#[derive(Debug, Clone, PartialEq)]
pub struct VisualMatch {
    pub compared: u32,
    pub matched: u32,
    pub mean_distance: f32,
    /// One of the two has at most two pages.
    pub tiny: bool,
    /// Every page pair compared in the chosen alignment, in sample order.
    pub pages: Vec<PagePair>,
}

/// Compares the sampled pages of `a` and `b`. `None` when they do not match
/// visually, by the rules in the module docs.
pub fn visual_match(a: &PdfFingerprint, b: &PdfFingerprint) -> Option<VisualMatch> {
    align(a, b).filter(|m| accepts(a, m))
}

/// Whether an alignment is enough to call two documents visually the same.
fn accepts(a: &PdfFingerprint, m: &VisualMatch) -> bool {
    if m.tiny {
        // Every page of a tiny document, and none of them blank.
        m.compared == a.page_count && m.matched == m.compared
    } else {
        m.compared >= MIN_COMPARED && m.matched as f32 >= MATCH_FRACTION * m.compared as f32
    }
}

/// The best alignment of the sampled pages of `a` and `b`, whether or not it
/// is good enough to be a match (the compare view shows the numbers of a pair
/// that does not match). `None` when the page counts are too far apart to
/// align at all.
pub fn align(a: &PdfFingerprint, b: &PdfFingerprint) -> Option<VisualMatch> {
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
        let mut pages: Vec<PagePair> = Vec::new();
        let mut matched = 0u32;
        let mut distance_sum = 0u32;
        for pa in a.pages.iter().filter(|p| !p.blank) {
            for pb in b.pages.iter().filter(|p| !p.blank) {
                let front = pa.index as i64 - pb.index as i64 == front_shift;
                let back = end(pa, a.page_count) - end(pb, b.page_count) == end_shift;
                if !(front || back) || seen.contains(&(pa.index, pb.index)) {
                    continue;
                }
                // A page too empty to compare says nothing either way.
                let Some(d) = page_distance(pa, pb) else { continue };
                seen.push((pa.index, pb.index));
                let is_match = d <= PAGE_MATCH_DISTANCE;
                if is_match {
                    matched += 1;
                    distance_sum += d;
                }
                pages.push(PagePair { a: pa.index, b: pb.index, distance: d, matched: is_match });
            }
        }
        let candidate = VisualMatch { compared: pages.len() as u32, matched, mean_distance: if matched > 0 { distance_sum as f32 / matched as f32 } else { 0.0 }, tiny, pages };
        if best.as_ref().is_none_or(|cur| candidate.matched > cur.matched) {
            best = Some(candidate);
        }
    }
    best
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

/// What was measured about a pair, which [`grade`] turns into a confidence and
/// the match's `detail` says in words.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Measured {
    /// Sample pages compared / matched in the alignment (0 for a text match,
    /// which has no visual match).
    pub compared: u32,
    pub matched: u32,
    /// Mean [`page_distance`] of the matched pages, of 256.
    pub mean_distance: f32,
    /// The text similarity the tier rests on, when there is text to compare:
    /// the plain Jaccard for a re-save, the distinctive Jaccard otherwise.
    pub text_similarity: Option<f32>,
    /// `|page count a - page count b|`.
    pub page_count_diff: u32,
}

impl Measured {
    fn from_alignment(v: &VisualMatch, a: &PdfFingerprint, b: &PdfFingerprint, text: Option<f32>) -> Measured {
        Measured { compared: v.compared, matched: v.matched, mean_distance: v.mean_distance, text_similarity: text, page_count_diff: a.page_count.abs_diff(b.page_count) }
    }
}

/// Which band a match is graded in. One more than [`PdfKind`]: an OCR variant
/// is graded differently when its two sides are two OCR runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    Resave,
    Ocr,
    OcrTwoLayers,
    Text,
}

impl Band {
    pub fn range(self) -> (f32, f32) {
        match self {
            Band::Resave => BAND_RESAVE,
            Band::Ocr => BAND_OCR,
            Band::OcrTwoLayers => BAND_OCR_TWO_LAYERS,
            Band::Text => BAND_TEXT,
        }
    }
}

fn unit(x: f32) -> f32 {
    if x.is_nan() {
        0.0
    } else {
        x.clamp(0.0, 1.0)
    }
}

/// `x` placed in `lo..=hi` as 0 to 1.
fn within(x: f32, lo: f32, hi: f32) -> f32 {
    unit((x - lo) / (hi - lo))
}

/// The confidence of a match in `band`, from what was measured, with the
/// tiny-document penalty if `tiny`. See "Graded confidence" in the module
/// docs: always within the band (times the penalty), never lower for a stronger
/// measurement.
pub fn grade(band: Band, m: &Measured, tiny: bool) -> f32 {
    let pages = if m.compared == 0 { 0.0 } else { within(m.matched as f32 / m.compared as f32, MATCH_FRACTION, 1.0) };
    let distance = unit(1.0 - m.mean_distance / PAGE_MATCH_DISTANCE as f32);
    let strength = match band {
        Band::Ocr => 0.5 * pages + 0.5 * distance,
        Band::Resave => match m.text_similarity {
            Some(j) => 0.4 * pages + 0.3 * distance + 0.3 * within(j, SAME_TEXT_JACCARD, 1.0),
            None => 0.5 * pages + 0.5 * distance,
        },
        Band::OcrTwoLayers => 0.4 * pages + 0.3 * distance + 0.3 * within(m.text_similarity.unwrap_or(DIFFERENT_TEXT_JACCARD), DIFFERENT_TEXT_JACCARD, SAME_TEXT_JACCARD),
        Band::Text => {
            let close = unit(1.0 - m.page_count_diff as f32 / MAX_PAGE_DIFF as f32);
            0.7 * within(m.text_similarity.unwrap_or(TEXT_MATCH_JACCARD), TEXT_MATCH_JACCARD, 1.0) + 0.3 * close
        }
    };
    let (lo, hi) = band.range();
    (lo + unit(strength) * (hi - lo)) * if tiny { TINY_PENALTY } else { 1.0 }
}

/// "7 of 8 sample pages match (mean page distance 12 of 256)".
fn pages_phrase(m: &Measured) -> String {
    format!("{} of {} sample pages match (mean page distance {:.0} of {})", m.matched, m.compared, m.mean_distance, SIGNATURE_BITS)
}

#[derive(Debug, Clone, PartialEq)]
pub struct PdfMatch {
    pub kind: PdfKind,
    /// Graded within the tier's band, see [`grade`].
    pub confidence: f32,
    pub detail: String,
    pub measured: Measured,
}

/// What a pair of fingerprints are, if anything, with no knowledge of the
/// rest of the library (so no boilerplate is recognised). The library-wide
/// path is [`classify_with`].
pub fn classify(a: &PdfFingerprint, b: &PdfFingerprint) -> Option<PdfMatch> {
    classify_with(a, b, &StopSlots::default())
}

/// [`classify`], leaving out the text boilerplate in `stops` wherever two
/// *different* texts are being judged to be one (two OCR layers; a text-only
/// match). A pair whose pages match and whose text is the same by the plain
/// Jaccard is a re-save and needs no such care.
pub fn classify_with(a: &PdfFingerprint, b: &PdfFingerprint, stops: &StopSlots) -> Option<PdfMatch> {
    let jac = jaccard(&a.minhash, &b.minhash);
    if let Some(v) = visual_match(a, b) {
        let counts = format!("page counts {} and {}", a.page_count, b.page_count);
        let (kind, band, text, what) = match (a.has_text, b.has_text) {
            (true, false) | (false, true) => (PdfKind::OcrVariant, Band::Ocr, None, "one has a text layer and the other is image-only".to_string()),
            (false, false) => (PdfKind::Resave, Band::Resave, None, "both are image-only".to_string()),
            (true, true) => match jac {
                Some(j) if j >= SAME_TEXT_JACCARD => (PdfKind::Resave, Band::Resave, Some(j), format!("text similarity {j:.2}")),
                // Two text layers over the same pages that are not the same
                // text: two OCR runs of one scan, or two documents on one
                // template. Only the words that are *not* library-wide
                // boilerplate can tell them apart.
                // Two OCR runs are runs over *images*: born-digital pages with
                // real text are never OCR variants of each other.
                _ if !(a.scanned && b.scanned) => return None,
                _ => match distinctive_jaccard(&a.minhash, &b.minhash, stops) {
                    Some((j, n)) if n >= MIN_DISTINCTIVE_SLOTS && j >= DIFFERENT_TEXT_JACCARD => (PdfKind::OcrVariant, Band::OcrTwoLayers, Some(j), format!("different text layers, text similarity {j:.2}")),
                    // The same look, other words: a template, not a copy.
                    _ => return None,
                },
            },
        };
        let measured = Measured::from_alignment(&v, a, b, text);
        let confidence = grade(band, &measured, v.tiny);
        return Some(PdfMatch { kind, confidence, detail: format!("{}; {what} ({counts})", pages_phrase(&measured)), measured });
    }
    let (j, n) = distinctive_jaccard(&a.minhash, &b.minhash, stops)?;
    let diff = a.page_count.abs_diff(b.page_count);
    let close = diff <= MAX_PAGE_DIFF;
    if !(a.has_text && b.has_text && close && n >= MIN_DISTINCTIVE_SLOTS && j >= TEXT_MATCH_JACCARD && a.text_chars >= TEXT_MATCH_MIN_CHARS && b.text_chars >= TEXT_MATCH_MIN_CHARS) {
        return None;
    }
    let measured = Measured { compared: 0, matched: 0, mean_distance: 0.0, text_similarity: Some(j), page_count_diff: diff };
    Some(PdfMatch { kind: PdfKind::TextMatch, confidence: grade(Band::Text, &measured, false), detail: format!("text similarity {j:.2} on different pages ({} and {} pages)", a.page_count, b.page_count), measured })
}

/// Everything known about how two fingerprinted PDFs compare, for the compare
/// view: the page alignment even when it is not a match, the text similarity,
/// and the verdict [`classify_with`] gives.
#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    /// The best page alignment (`None` when the page counts are too far apart
    /// to align at all).
    pub alignment: Option<VisualMatch>,
    /// Whether that alignment is good enough to be a visual match.
    pub visual_match: bool,
    /// The plain MinHash Jaccard of the two text layers.
    pub text_jaccard: Option<f32>,
    /// What the library's duplicate scan would say about the pair.
    pub found: Option<PdfMatch>,
}

pub fn compare(a: &PdfFingerprint, b: &PdfFingerprint, stops: &StopSlots) -> Comparison {
    let alignment = align(a, b);
    let visual = alignment.as_ref().is_some_and(|m| accepts(a, m));
    Comparison { alignment, visual_match: visual, text_jaccard: jaccard(&a.minhash, &b.minhash), found: classify_with(a, b, stops) }
}

// ---------------------------------------------------------------------
// The candidate index
// ---------------------------------------------------------------------

const BANDS: usize = 16;
const ROWS: usize = MINHASH_LEN / BANDS;
/// Bytes a page takes in the sidecar: index, blank flag, bits, trusted mask.
const PAGE_RECORD: usize = 4 + 1 + 32 + 32;

/// What [`find_pairs`] did, for the scale tests and the logs.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IndexStats {
    pub fingerprints: usize,
    /// Pairs the index proposed.
    pub candidates: usize,
    /// Pairs that were verified as a match.
    pub matches: usize,
    /// Groups of colliding entries skipped for being larger than
    /// [`MAX_POSTING`].
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

/// Which [`TABLE_BITS`] of the 256 signature bits each of the [`TABLES`]
/// tables samples. Fixed, so an index built later agrees with one built now.
fn table_layout() -> Vec<[u8; TABLE_BITS]> {
    (0..TABLES)
        .map(|t| {
            let mut chosen = [0u8; TABLE_BITS];
            let mut used = [false; 256];
            let mut n = 0;
            let mut counter = 0u64;
            while n < TABLE_BITS {
                let bit = (splitmix(t as u64 * 7919 + counter) % 256) as usize;
                counter += 1;
                if !used[bit] {
                    used[bit] = true;
                    chosen[n] = bit as u8;
                    n += 1;
                }
            }
            chosen
        })
        .collect()
}

/// A page's key in one table: the sampled bits, or `None` if any of them is
/// untrusted (a key with a coin-flip bit in it would not match its own copy).
fn page_key(page: &PageHash, positions: &[u8; TABLE_BITS]) -> Option<u64> {
    let mut key = 0u64;
    for (n, &bit) in positions.iter().enumerate() {
        let (word, shift) = (bit as usize / 64, bit as usize % 64);
        if page.mask[word] >> shift & 1 == 0 {
            return None;
        }
        key |= (page.bits[word] >> shift & 1) << n;
    }
    Some(key)
}

struct Entry {
    key: u64,
    pc: u32,
    id: u32,
    slot: u16,
    /// The sampled page's index (0 for a MinHash band).
    page: u32,
}

/// Records, in `hits`, every pair of entries that share a key, whose page
/// counts are within [`MAX_PAGE_DIFF`] and of which the lower is `anchor`:
/// the pair `(lower id, higher id)` maps to a bitmask of the lower id's slots
/// that collided. A group of more than [`MAX_POSTING`] colliding entries is a
/// key that says nothing (a near-empty page) and is skipped.
fn collide(mut entries: Vec<Entry>, anchor: u32, hits: &mut HashMap<(u32, u32), u16>, skipped: &mut usize) {
    entries.sort_unstable_by_key(|e| (e.key, e.pc, e.id));
    let mut start = 0;
    while start < entries.len() {
        let mut end = start + 1;
        while end < entries.len() && entries[end].key == entries[start].key {
            end += 1;
        }
        let group = &entries[start..end];
        start = end;
        if group.len() > MAX_POSTING {
            *skipped += 1;
            continue;
        }
        for (i, a) in group.iter().enumerate() {
            if a.pc != anchor {
                continue;
            }
            for b in &group[i + 1..] {
                if b.pc - a.pc > MAX_PAGE_DIFF {
                    break;
                }
                // A copy collides on its own page, give or take the shift a
                // changed page count allows; any other collision is chance.
                if a.id == b.id || a.page.abs_diff(b.page) > MAX_PAGE_DIFF {
                    continue;
                }
                let (lo, hi, slot) = if a.id < b.id { (a.id, b.id, a.slot) } else { (b.id, a.id, b.slot) };
                *hits.entry((lo, hi)).or_insert(0) |= 1 << slot;
            }
        }
    }
}

/// Every matching pair among `fps`, by the index and then verification.
/// Work grows with the number of *candidate* pairs, not with `n^2`.
///
/// Fingerprints are taken a page count at a time (with the two counts above
/// it, which is how far a scan may differ), so the working set is a few
/// hundred documents however large the library: for each of [`TABLES`] tables,
/// every non-blank sampled page whose [`TABLE_BITS`] sampled bits are all
/// trusted is keyed by them, and pages that share a key are paired.
pub fn find_pairs(fps: &[PdfFingerprint]) -> (Vec<PdfPair>, IndexStats) {
    use std::collections::BTreeMap;
    let mut stats = IndexStats { fingerprints: fps.len(), ..Default::default() };
    let layout = table_layout();
    let stops = StopSlots::from_fingerprints(fps);

    let mut by_count: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, fp) in fps.iter().enumerate() {
        by_count.entry(fp.page_count).or_default().push(i);
    }

    let mut pairs = Vec::new();
    for &anchor in by_count.keys() {
        let window: Vec<usize> = by_count.range(anchor..=anchor + MAX_PAGE_DIFF).flat_map(|(_, ids)| ids.iter().copied()).collect();
        let mut hits: HashMap<(u32, u32), u16> = HashMap::new();

        for positions in &layout {
            let mut entries = Vec::new();
            for &i in &window {
                let fp = &fps[i];
                for (slot, page) in fp.pages.iter().filter(|p| !p.blank).enumerate() {
                    if let Some(key) = page_key(page, positions) {
                        entries.push(Entry { key, pc: fp.page_count, id: i as u32, slot: slot as u16, page: page.index });
                    }
                }
            }
            collide(entries, anchor, &mut hits, &mut stats.skipped_postings);
        }
        for band in 0..BANDS {
            let entries: Vec<Entry> = window.iter().filter(|&&i| fps[i].minhash.len() == MINHASH_LEN).map(|&i| Entry { key: band_key(&fps[i].minhash, band), pc: fps[i].page_count, id: i as u32, slot: 8, page: 0 }).collect();
            collide(entries, anchor, &mut hits, &mut stats.skipped_postings);
        }

        let mut candidates: Vec<((u32, u32), u16)> = hits.into_iter().collect();
        candidates.sort_unstable_by_key(|(pair, _)| *pair);
        for ((lo, hi), mask) in candidates {
            let (a, b) = (&fps[lo as usize], &fps[hi as usize]);
            let needed = MIN_CANDIDATE_PAGES.min(a.usable_pages().min(b.usable_pages()) as u32).max(1);
            if (mask & 0xFF).count_ones() < needed && mask & 0x100 == 0 {
                continue;
            }
            stats.candidates += 1;
            if let Some(found) = classify_with(a, b, &stops) {
                stats.matches += 1;
                pairs.push(PdfPair { a: lo as usize, b: hi as usize, found });
            }
        }
    }
    pairs.sort_by_key(|p| (p.a, p.b));
    (pairs, stats)
}

// ---------------------------------------------------------------------
// The fingerprint job
// ---------------------------------------------------------------------

/// Most files read at once **in this process**: **one**. (With the
/// out-of-process worker pool, [`crate::pdf_worker`], there are as many as the
/// pool has workers; each has a PDFium of its own.)
///
/// PDFium is not safe for concurrent use of documents, whatever the
/// `pdfium-render` `thread_safe` feature suggests. With two workers the whole
/// `lexmechanic_srv` process died within seconds on a real library (about 9,000
/// PDFs), silently, in native code. One thread fingerprinted all 8,952 of them
/// without trouble, and the server job with one worker finished 8,812 PDFs in
/// 67 minutes. Every PDFium call in the process is now also serialised behind
/// one global lock (`lexmechanic_ebooks::pdf::rasterize`), so more workers could
/// overlap only the hashing anyway.
pub const MAX_FINGERPRINT_WORKERS: usize = 1;

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
///
/// # Worker processes and poison files
///
/// When a PDF worker binary is available ([`crate::pdf_worker`]) the files are
/// read by a pool of worker processes, in parallel. A worker that crashes or
/// hangs on a file is replaced, and the file is recorded as failed with the
/// reason `crashed` or `timed out` (not retried until the version changes).
/// Without a worker binary the files are read in this process, one at a time.
///
/// In both modes a file is marked *in flight* in the database before it is
/// read. If the whole process dies, the next run finds the marks and treats
/// those files as **suspects**, not failures: the process may have died on any
/// one of them (or on none, if it was simply stopped). Suspects are read first,
/// one at a time, each in a fresh single worker (in this process when there is
/// no pool), marked `fingerprint-suspect` before it is sent. One that crashes
/// or times out on its own is recorded as such; one still marked
/// `fingerprint-suspect` at the next start killed the process on its own
/// retry, and is recorded as crashed without a third try.
pub fn fingerprint_pdfs(cache: &Cache, options: &PipelineOptions, control: &JobControl) -> Result<FingerprintReport> {
    use std::sync::atomic::Ordering;
    control.set_phase("fingerprinting");
    let started = std::time::Instant::now();
    let library = cache.backend.library_path.clone();
    let store = cache.checksums();
    let mut report = FingerprintReport::default();

    // Files a previous run was reading when the process died. Recorded as
    // crashed, so this run does not read them again.
    for hash in store.take_inflight("fingerprint-suspect")? {
        let why = "crashed: this PDF was being read alone when the process died, so it was not tried again".to_string();
        store.record_pdf_fingerprint(&hash, &Err(why.clone()))?;
        control.errors.fetch_add(1, Ordering::Relaxed);
        report.failed_count += 1;
        if report.failed.len() < 100 {
            report.failed.push((format!("(content {hash})"), why));
        }
    }

    let suspects: std::collections::HashSet<String> = store.take_inflight("fingerprint")?.into_iter().collect();

    let todo = store.pdf_fingerprints_pending()?;
    control.total.store(todo.len() as u64, Ordering::Relaxed);
    report.total = todo.len();
    if todo.is_empty() {
        return Ok(report);
    }
    let pool = options.pdf_pool.resolve(options.pdf_workers);
    if pool.is_none() && !lexmechanic_ebooks::pdf::rasterize::is_available() {
        bail!("PDF fingerprinting needs the PDFium library, which was not found (run `cargo xtask fetch-pdfium` or set {})", lexmechanic_ebooks::pdf::rasterize::LIBRARY_PATH_VAR);
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

    let workers = match &pool {
        Some(pool) => pool.workers(),
        None => options.workers.clamp(1, MAX_FINGERPRINT_WORKERS),
    };
    let mut abort: Option<anyhow::Error> = None;
    // Suspects first, one to a batch; then everything else.
    let (suspect_items, rest): (Vec<_>, Vec<_>) = by_hash.into_iter().partition(|(hash, _)| suspects.contains(hash));
    let mut batches: Vec<(&[(String, Vec<(String, String, String)>)], bool)> = suspect_items.chunks(1).map(|c| (c, true)).collect();
    batches.extend(rest.chunks(options.batch_size.max(1)).map(|c| (c, false)));
    for (chunk, suspect) in batches {
        if control.is_cancelled() {
            break;
        }
        let kind = if suspect { "fingerprint-suspect" } else { "fingerprint" };
        let workers = if suspect { 1 } else { workers };
        let batch_pool: Option<std::sync::Arc<crate::pdf_worker::Pool>> = if suspect { pool.as_ref().map(|p| std::sync::Arc::new(p.isolated())) } else { pool.clone() };
        let results = parallel_map(chunk, workers, control, |(hash, locations)| -> Result<Option<(String, Result<PdfFingerprint>)>> {
            for (folder, name, format) in locations {
                let Some(path) = format_file_path(&library, folder, name, format) else { continue };
                let display = path.display().to_string();
                store.mark_inflight(kind, hash)?;
                let outcome = match &batch_pool {
                    Some(pool) => fingerprint_via_pool(pool, &path),
                    None => fingerprint_file(&path),
                };
                if let Err(e) = &outcome {
                    // A crash or a hang is recorded the moment it happens, so
                    // it survives whatever happens to this run next.
                    if !is_unavailable(e) && is_worker_failure(e) {
                        store.record_pdf_fingerprint(hash, &Err(format!("{e:#}")))?;
                    }
                }
                store.clear_inflight(kind, hash)?;
                return match outcome {
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
    control.add_time("fingerprint", started.elapsed());
    report.cancelled = control.is_cancelled();
    Ok(report)
}

/// Marker for the errors [`fingerprint_via_pool`] makes from a worker that
/// died or hung: they are the file's fault, unlike "no worker".
#[derive(Debug)]
struct WorkerFailure(String);

impl std::fmt::Display for WorkerFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WorkerFailure {}

fn is_worker_failure(error: &anyhow::Error) -> bool {
    error.downcast_ref::<WorkerFailure>().is_some()
}

/// [`fingerprint_file`] through a worker process. A missing PDFium in the
/// worker is [`is_unavailable`]; a worker that dies or hangs is a
/// [`WorkerFailure`] whose text is `crashed: ...` or `timed out: ...`.
pub fn fingerprint_via_pool(pool: &crate::pdf_worker::Pool, path: &Path) -> Result<PdfFingerprint> {
    use crate::pdf_worker::{Reply, Request};
    let reply = pool.run(Request { id: 0, path: path.to_string_lossy().to_string(), cover_width: None, fingerprint: true });
    match reply {
        Reply::Done(resp) => {
            if resp.unavailable {
                return Err(anyhow::Error::new(lexmechanic_ebooks::pdf::rasterize::RasterizeError::Unavailable { searched: "the PDF worker's PDFium".to_string(), var: lexmechanic_ebooks::pdf::rasterize::LIBRARY_PATH_VAR }));
            }
            if let Some(error) = resp.error {
                bail!("{error}");
            }
            match (resp.fingerprint, resp.fingerprint_error) {
                (Some(fp), _) => Ok(fp),
                (None, Some(why)) => bail!("{why}"),
                (None, None) => bail!("the PDF worker returned no fingerprint"),
            }
        }
        reply @ (Reply::Crashed(_) | Reply::TimedOut) => Err(anyhow::Error::new(WorkerFailure(reply.failure_reason().unwrap_or_default()))),
        Reply::SpawnFailed(why) => Err(anyhow::Error::new(NoWorker(why))),
    }
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
