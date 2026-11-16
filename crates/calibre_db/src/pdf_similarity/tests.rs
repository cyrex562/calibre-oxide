//! Tests for [`super`]. The hashing, matching and indexing are tested on
//! synthetic pages and fingerprints, with no PDFium; the tests that need it
//! skip when it is absent, as `rasterize`'s do.

use super::*;
use crate::scan_pipeline::{JobControl, PipelineOptions};

// ---------------------------------------------------------------------
// Synthetic pages
// ---------------------------------------------------------------------

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = splitmix(self.0);
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

const W: usize = 256;
const H: usize = 330;

fn fill(img: &mut [u8], x0: usize, y0: usize, x1: usize, y1: usize, v: u8) {
    for y in y0..y1.min(H) {
        for x in x0..x1.min(W) {
            img[y * W + x] = v;
        }
    }
}

/// A page of "text": a heading, paragraphs of word-shaped blocks, ragged last
/// lines, and sometimes a picture. Different seeds give different layouts.
fn page(seed: u64) -> Vec<u8> {
    let mut r = Rng(seed.wrapping_mul(0x9E37_79B9).wrapping_add(7));
    let mut img = vec![238u8; W * H];
    let (left, right) = (28 + r.below(10) as usize, W - 28 - r.below(10) as usize);
    let mut y = 22 + r.below(14) as usize;
    // Heading.
    let hw = 40 + r.below(90) as usize;
    let hx = left + r.below(((right - left).saturating_sub(hw)) as u64 + 1) as usize;
    fill(&mut img, hx, y, hx + hw, y + 6, 20);
    y += 20;
    if r.below(3) == 0 {
        let (pw, ph) = (60 + r.below(100) as usize, 40 + r.below(70) as usize);
        let px = left + r.below(((right - left).saturating_sub(pw)) as u64 + 1) as usize;
        fill(&mut img, px, y, px + pw, y + ph, 110 + r.below(60) as u8);
        y += ph + 10;
    }
    while y < H - 30 {
        let lines = 3 + r.below(9) as usize;
        for l in 0..lines {
            if y >= H - 30 {
                break;
            }
            let last = l == lines - 1;
            let indent = if l == 0 { 8 + r.below(6) as usize } else { 0 };
            let end = if last { left + (right - left) * (20 + r.below(70) as usize) / 100 } else { right };
            let mut x = left + indent;
            while x + 4 < end {
                let w = 4 + r.below(16) as usize;
                fill(&mut img, x, y, (x + w).min(end), y + 3, 45);
                x += w + 3;
            }
            y += 6;
        }
        y += 6 + r.below(8) as usize;
    }
    img
}

fn noisy(img: &[u8], r: &mut Rng, sigma: f32) -> Vec<u8> {
    img.iter()
        .map(|&v| {
            let n = (r.unit() + r.unit() + r.unit() - 1.5) * 2.0 * sigma;
            (v as f32 + n).clamp(0.0, 255.0) as u8
        })
        .collect()
}

/// A scanner's copy: the page shifted a little, with a dark border and noise,
/// JPEG-compressed.
fn scan_of(img: &[u8], seed: u64, dx: i32, dy: i32, border: bool, quality: u8) -> Vec<u8> {
    let mut r = Rng(seed);
    let mut out = vec![200u8; W * H];
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            let (sx, sy) = (x - dx, y - dy);
            if (0..W as i32).contains(&sx) && (0..H as i32).contains(&sy) {
                out[y as usize * W + x as usize] = img[sy as usize * W + sx as usize];
            }
        }
    }
    if border {
        fill(&mut out, 0, 0, W, 5, 25);
        fill(&mut out, 0, H - 7, W, H, 25);
        fill(&mut out, 0, 0, 6, H, 25);
        fill(&mut out, W - 4, 0, W, H, 25);
    }
    jpeg(&noisy(&out, &mut r, 12.0), quality)
}

fn jpeg(img: &[u8], quality: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality).encode(img, W as u32, H as u32, image::ColorType::L8).unwrap();
    image::load_from_memory(&bytes).unwrap().to_luma8().into_raw()
}

/// What an optimizer does: downsample, recompress hard.
fn optimized(img: &[u8], quality: u8) -> Vec<u8> {
    let buf = image::GrayImage::from_raw(W as u32, H as u32, img.to_vec()).unwrap();
    let small = image::imageops::resize(&buf, (W as f32 * 0.6) as u32, (H as f32 * 0.6) as u32, image::imageops::FilterType::Triangle);
    let back = image::imageops::resize(&small, W as u32, H as u32, image::imageops::FilterType::Triangle);
    jpeg(back.as_raw(), quality)
}

fn hash(img: &[u8]) -> Option<u64> {
    page_hash(img, W as u32, H as u32)
}

// ---------------------------------------------------------------------
// The page hash
// ---------------------------------------------------------------------

#[test]
fn blank_pages_have_no_hash() {
    assert_eq!(hash(&vec![255u8; W * H]), None, "white page");
    assert_eq!(hash(&vec![20u8; W * H]), None, "uniformly dark page");
    let mut r = Rng(1);
    assert_eq!(hash(&noisy(&vec![235u8; W * H], &mut r, 12.0)), None, "a blank scan: paper grain only");
    // A page holding only a page number.
    let mut img = vec![238u8; W * H];
    fill(&mut img, 122, 300, 130, 308, 40);
    assert_eq!(hash(&img), None, "page-number-only page");
    // A blank scan with a dark border.
    let mut img = vec![235u8; W * H];
    fill(&mut img, 0, 0, W, 5, 25);
    fill(&mut img, 0, 0, 6, H, 25);
    assert_eq!(hash(&noisy(&img, &mut r, 8.0)), None, "blank page with a scanner border");
}

#[test]
fn a_text_page_has_a_hash() {
    assert!(hash(&page(1)).is_some());
}

/// The evidence behind [`PAGE_MATCH_DISTANCE`]: Hamming distances of one page
/// against its own scans and optimizer re-saves, and of different pages. Run
/// with `--nocapture` to see them.
#[test]
fn same_pages_are_close_and_different_pages_are_far() {
    let (mut same, mut diff) = (Vec::new(), Vec::new());
    let mut r = Rng(99);
    for seed in 0..80u64 {
        let original = page(seed);
        let h = hash(&original).unwrap();
        let variants = [
            scan_of(&original, seed + 1000, 0, 0, false, 70),
            scan_of(&original, seed + 2000, 3, -2, true, 60),
            scan_of(&original, seed + 3000, -4, 4, true, 80),
            optimized(&original, 40),
            optimized(&scan_of(&original, seed + 4000, 2, 2, true, 85), 30),
        ];
        for v in &variants {
            same.push(hamming(h, hash(v).expect("a variant of a text page is not blank")));
        }
        let other = hash(&page(seed + 5000 + r.below(1000))).unwrap();
        diff.push(hamming(h, other));
    }
    same.sort_unstable();
    diff.sort_unstable();
    let pct = |v: &[u32], p: usize| v[(v.len() - 1) * p / 100];
    eprintln!("same page:      n={} median={} p90={} p99={} max={}", same.len(), pct(&same, 50), pct(&same, 90), pct(&same, 99), same.last().unwrap());
    eprintln!("different page: n={} min={} p1={} p10={} median={}", diff.len(), diff[0], pct(&diff, 1), pct(&diff, 10), pct(&diff, 50));
    let within = |v: &[u32]| v.iter().filter(|&&d| d <= PAGE_MATCH_DISTANCE).count();
    assert!(within(&same) * 100 >= same.len() * 95, "{} of {} same-page pairs within {PAGE_MATCH_DISTANCE}", within(&same), same.len());
    assert!(within(&diff) * 100 <= diff.len() * 3, "{} of {} different-page pairs within {PAGE_MATCH_DISTANCE}", within(&diff), diff.len());
}

// ---------------------------------------------------------------------
// Sampling, MinHash, encoding
// ---------------------------------------------------------------------

#[test]
fn sampling_is_first_last_and_middle() {
    assert!(sample_indices(0).is_empty());
    assert_eq!(sample_indices(1), vec![0]);
    assert_eq!(sample_indices(8), (0..8).collect::<Vec<_>>());
    assert_eq!(sample_indices(9), vec![0, 1, 2, 3, 6, 6, 7, 8].into_iter().collect::<std::collections::BTreeSet<_>>().into_iter().collect::<Vec<_>>());
    let s = sample_indices(300);
    assert_eq!(s, vec![0, 1, 2, 100, 200, 297, 298, 299]);
    assert!(s.len() <= SAMPLE_PAGES);
    assert_eq!(text_page_indices(10), (0..10).collect::<Vec<_>>());
    assert_eq!(text_page_indices(1000).len(), TEXT_PAGES);
}

fn lorem(seed: u64, n: usize) -> Vec<String> {
    let mut r = Rng(seed);
    (0..n).map(|_| format!("w{}", r.below(3000))).collect()
}

#[test]
fn minhash_estimates_similarity() {
    let a = lorem(1, 5000);
    let mut b = a.clone();
    let mut r = Rng(5);
    for _ in 0..250 {
        let i = r.below(b.len() as u64) as usize;
        b[i] = format!("typo{i}");
    }
    let c = lorem(2, 5000);
    let (ma, mb, mc) = (minhash(&a), minhash(&b), minhash(&c));
    assert_eq!(ma.len(), MINHASH_LEN);
    assert_eq!(jaccard(&ma, &ma), Some(1.0));
    let close = jaccard(&ma, &mb).unwrap();
    let far = jaccard(&ma, &mc).unwrap();
    assert!((0.6..0.95).contains(&close), "5% of words changed: {close}");
    assert!(far < 0.05, "unrelated text: {far}");
    assert!(minhash(&lorem(3, 2)).is_empty(), "under three words");
    assert_eq!(jaccard(&[], &mb), None);
}

#[test]
fn words_fold_case_accents_and_punctuation() {
    assert_eq!(words("The Café, “Naïve” — OK!"), vec!["the", "cafe", "naive", "ok"]);
}

#[test]
fn a_fingerprint_survives_its_encoding() {
    let f = fp(7, 120, true, 1);
    assert_eq!(PdfFingerprint::decode_pages(&f.encode_pages()).unwrap(), f.pages);
    assert_eq!(PdfFingerprint::decode_minhash(&f.encode_minhash()), f.minhash);
    assert!(PdfFingerprint::decode_pages(&[1, 2, 3]).is_none());
}

// ---------------------------------------------------------------------
// Synthetic fingerprints
// ---------------------------------------------------------------------

/// A fingerprint of a document of `pages` pages whose sampled pages are
/// random (seeded: `seed` is the book), with text from `seed` if `text`.
fn fp(seed: u64, pages: u32, text: bool, _salt: u64) -> PdfFingerprint {
    let mut r = Rng(seed.wrapping_mul(0x1234_5678_9ABC_DEF1) | 1);
    let hashes: Vec<PageHash> = sample_indices(pages as usize).into_iter().map(|i| PageHash { index: i as u32, hash: r.next(), blank: false }).collect();
    let (has_text, text_chars, minhash) = if text { (true, 9000, super::minhash(&lorem(seed, 3000))) } else { (false, 0, Vec::new()) };
    PdfFingerprint { page_count: pages, pages: hashes, text_chars, has_text, minhash }
}

/// The same book re-rendered: every hash gets up to `bits` bits flipped.
fn rerendered(f: &PdfFingerprint, seed: u64, bits: u32) -> PdfFingerprint {
    let mut r = Rng(seed | 1);
    let mut g = f.clone();
    for p in g.pages.iter_mut().filter(|p| !p.blank) {
        for _ in 0..r.below(bits as u64 + 1) {
            p.hash ^= 1 << r.below(64);
        }
    }
    g
}

fn blank_all(f: &PdfFingerprint) -> PdfFingerprint {
    let mut g = f.clone();
    for p in g.pages.iter_mut() {
        p.blank = true;
        p.hash = 0;
    }
    g
}

/// `f` as it would be if a cover page were added at the front: every
/// sampled page of the original is one later, the page count is one more.
fn with_cover_added(f: &PdfFingerprint, cover_seed: u64) -> PdfFingerprint {
    let n = f.page_count + 1;
    let mut r = Rng(cover_seed | 1);
    // Re-sample for the longer document, taking the original's hash for
    // every position that is the original's page `i - 1` and a fresh hash
    // for the rest (a page the original sampled nothing of).
    let known: std::collections::HashMap<u32, u64> = f.pages.iter().map(|p| (p.index + 1, p.hash)).collect();
    let pages = sample_indices(n as usize).into_iter().map(|i| PageHash { index: i as u32, hash: known.get(&(i as u32)).copied().unwrap_or_else(|| r.next()), blank: false }).collect();
    PdfFingerprint { page_count: n, pages, ..f.clone() }
}

// ---------------------------------------------------------------------
// Visual matching
// ---------------------------------------------------------------------

#[test]
fn a_re_render_matches_and_a_different_book_does_not() {
    let a = fp(1, 200, false, 0);
    assert!(visual_match(&a, &rerendered(&a, 3, 6)).is_some());
    // Same page count, different pages.
    for seed in 2..200 {
        assert!(visual_match(&a, &fp(seed, 200, false, 0)).is_none(), "book {seed} matched");
    }
}

#[test]
fn a_page_count_off_by_more_than_two_never_matches() {
    let a = fp(1, 200, false, 0);
    let mut b = a.clone();
    b.page_count = 203;
    assert!(visual_match(&a, &b).is_none());
}

#[test]
fn an_added_cover_is_aligned_by_shifting() {
    let a = fp(11, 200, false, 0);
    let b = with_cover_added(&a, 5);
    assert_eq!(b.page_count, 201);
    let m = visual_match(&a, &b).expect("a cover added to the front shifts the front pages, not the end");
    assert!(m.matched >= 3, "{m:?}");
    // And the other way round.
    assert!(visual_match(&b, &a).is_some());
}

#[test]
fn a_short_document_needs_every_page_and_equal_counts() {
    let a = fp(21, 2, false, 0);
    assert!(visual_match(&a, &rerendered(&a, 4, 4)).is_some());
    assert!(visual_match(&a, &fp(22, 2, false, 0)).is_none());
    // One page right, one wrong.
    let mut b = a.clone();
    b.pages[1].hash = !b.pages[1].hash;
    assert!(visual_match(&a, &b).is_none());
    // A page count of 3 against 2.
    let c = fp(21, 3, false, 0);
    assert!(visual_match(&a, &c).is_none());
    // A blank page in a two-page document disqualifies it.
    let mut d = a.clone();
    d.pages[1].blank = true;
    assert!(visual_match(&d, &d.clone()).is_none());
}

#[test]
fn blank_pages_are_never_evidence() {
    // Two unrelated books that both open and close on blanks: only blank
    // pages "match", and blank pages are not compared at all.
    let a = blank_all(&fp(1, 200, false, 0));
    let b = blank_all(&fp(2, 200, false, 0));
    assert!(visual_match(&a, &b).is_none());
    // Mostly blank: the two pages that are not blank agree, which is fewer
    // than the three it takes.
    let real = fp(3, 200, false, 0);
    let mut sparse = blank_all(&real);
    sparse.pages[0] = real.pages[0];
    sparse.pages[7] = real.pages[7];
    assert!(visual_match(&sparse, &rerendered(&sparse, 9, 4)).is_none());
    // Not every page of a book, but three real ones are enough.
    let mut some = blank_all(&real);
    for i in [0, 3, 7] {
        some.pages[i] = real.pages[i];
    }
    assert!(visual_match(&some, &rerendered(&some, 9, 4)).is_some());
}

// ---------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------

fn with_text(f: &PdfFingerprint, seed: u64) -> PdfFingerprint {
    PdfFingerprint { has_text: true, text_chars: 9000, minhash: super::minhash(&lorem(seed, 3000)), ..f.clone() }
}

#[test]
fn image_only_against_text_layer_is_an_ocr_variant() {
    let scan = fp(1, 300, false, 0);
    let ocr = with_text(&rerendered(&scan, 5, 6), 1);
    let m = classify(&scan, &ocr).unwrap();
    assert_eq!(m.kind, PdfKind::OcrVariant);
    assert_eq!(m.confidence, CONFIDENCE_OCR);
    assert_eq!(classify(&ocr, &scan).unwrap().kind, PdfKind::OcrVariant);
}

#[test]
fn the_same_pages_and_the_same_text_is_a_resave() {
    let a = with_text(&fp(2, 300, false, 0), 2);
    let b = rerendered(&a, 6, 6);
    assert_eq!(classify(&a, &b).unwrap().kind, PdfKind::Resave);
    // Both image-only.
    let c = fp(3, 300, false, 0);
    let m = classify(&c, &rerendered(&c, 7, 6)).unwrap();
    assert_eq!((m.kind, m.confidence), (PdfKind::Resave, CONFIDENCE_RESAVE));
}

#[test]
fn two_different_ocr_runs_are_an_ocr_variant_with_less_confidence() {
    let base = fp(4, 300, false, 0);
    let a = with_text(&base, 10);
    let mut words_b = lorem(10, 3000);
    for (i, w) in words_b.iter_mut().enumerate() {
        if i % 12 == 0 {
            *w = format!("ocr{i}");
        }
    }
    let b = PdfFingerprint { minhash: minhash(&words_b), ..rerendered(&a, 8, 5) };
    let j = jaccard(&a.minhash, &b.minhash).unwrap();
    assert!((DIFFERENT_TEXT_JACCARD..SAME_TEXT_JACCARD).contains(&j), "{j}");
    let m = classify(&a, &b).unwrap();
    assert_eq!((m.kind, m.confidence), (PdfKind::OcrVariant, CONFIDENCE_OCR_TWO_LAYERS));
}

#[test]
fn the_same_look_with_different_words_is_not_a_match() {
    let base = fp(5, 300, false, 0);
    let a = with_text(&base, 20);
    let b = with_text(&rerendered(&base, 9, 4), 21);
    assert!(classify(&a, &b).is_none());
}

#[test]
fn the_same_words_on_different_pages_is_a_text_match() {
    let a = with_text(&fp(6, 300, false, 0), 30);
    let b = PdfFingerprint { page_count: 301, pages: fp(7, 301, false, 0).pages, ..with_text(&fp(7, 301, false, 0), 30) };
    let m = classify(&a, &b).unwrap();
    assert_eq!((m.kind, m.confidence), (PdfKind::TextMatch, CONFIDENCE_TEXT));
    // Not when the page counts are far apart, or the text is short.
    let far = PdfFingerprint { page_count: 320, ..b.clone() };
    assert!(classify(&a, &far).is_none());
    let short = PdfFingerprint { text_chars: 300, ..b };
    assert!(classify(&a, &short).is_none());
}

#[test]
fn a_tiny_document_is_less_confident() {
    let a = fp(8, 2, false, 0);
    let m = classify(&a, &rerendered(&a, 2, 3)).unwrap();
    assert!((m.confidence - CONFIDENCE_RESAVE * TINY_PENALTY).abs() < 1e-6);
}

// ---------------------------------------------------------------------
// The index
// ---------------------------------------------------------------------

#[test]
fn the_index_finds_copies_among_unrelated_books() {
    let mut fps: Vec<PdfFingerprint> = (0..500).map(|s| fp(1000 + s, 150 + (s % 7) as u32, s % 3 == 0, 0)).collect();
    // Book 10 re-saved; book 20 scanned and OCR'd; book 30 with a cover added.
    fps.push(rerendered(&fps[10], 1, 5));
    fps.push(with_text(&rerendered(&fps[20], 2, 5), 77)); // 20 has no text
    let cover = with_cover_added(&fps[30], 3);
    fps.push(cover);
    let (pairs, stats) = find_pairs(&fps);
    let find = |a: usize, b: usize| pairs.iter().find(|p| (p.a, p.b) == (a.min(b), a.max(b)));
    assert!(find(10, 500).is_some(), "re-save not found: {pairs:?}");
    assert!(find(20, 501).is_some(), "re-OCR not found");
    assert!(find(30, 502).is_some(), "cover-added not found");
    assert_eq!(pairs.len(), 3, "unexpected pairs: {pairs:?}");
    assert!(stats.candidates < 100, "{stats:?}");
}

/// The index must not be all-pairs. 3,000 fingerprints is 4.5 million pairs.
#[test]
fn candidates_are_a_tiny_fraction_of_all_pairs() {
    let fps: Vec<PdfFingerprint> = (0..3000).map(|s| fp(50_000 + s, 200 + (s % 5) as u32, s % 2 == 0, 0)).collect();
    let (pairs, stats) = find_pairs(&fps);
    assert!(pairs.is_empty());
    assert!(stats.candidates < 3000, "{stats:?}");
}

/// 20,000 random fingerprints in page-count blocks of realistic size, with
/// planted copies. Ignored for the time it takes in a debug build; the
/// numbers are in the PR. `cargo test -p calibre_db --lib --release pdf_similarity -- --ignored --nocapture`
#[test]
#[ignore]
fn scale_20k_fingerprints() {
    let n = 20_000usize;
    let mut fps: Vec<PdfFingerprint> = (0..n as u64).map(|s| fp(900_000 + s, 80 + (s % 400) as u32, s % 2 == 0, 0)).collect();
    let planted: Vec<usize> = (0..200).map(|i| i * 97).collect();
    for (k, &i) in planted.iter().enumerate() {
        let copy = if k % 2 == 0 { rerendered(&fps[i], k as u64 + 1, 6) } else { with_text(&rerendered(&fps[i], k as u64 + 1, 6), 10_000 + k as u64) };
        fps.push(copy);
    }
    let start = std::time::Instant::now();
    let (pairs, stats) = find_pairs(&fps);
    let elapsed = start.elapsed();
    eprintln!("{} fingerprints: {elapsed:?}, {stats:?}, {} pairs (all-pairs would be {})", fps.len(), pairs.len(), fps.len() * (fps.len() - 1) / 2);
    assert!(elapsed.as_secs() < 60);
    assert!(stats.candidates < fps.len() * 20);
    for (k, &i) in planted.iter().enumerate() {
        assert!(pairs.iter().any(|p| (p.a, p.b) == (i, n + k)), "planted copy {k} of {i} not found");
    }
    assert_eq!(pairs.len(), planted.len(), "false pairs among {n} random books");
}

// ---------------------------------------------------------------------
// The cache and the job
// ---------------------------------------------------------------------

fn library() -> (tempfile::TempDir, Cache) {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path()).unwrap();
    (dir, cache)
}

fn add_pdf(dir: &std::path::Path, cache: &Cache, name: &str, bytes: &[u8]) -> i32 {
    let source = dir.join(format!("src-{name}.pdf"));
    std::fs::write(&source, bytes).unwrap();
    let mut meta = calibre_ebooks::metadata::MetaInformation::default();
    meta.title = name.to_string();
    meta.authors = vec![format!("Author {name}")];
    cache.add_book(&source, &meta).unwrap()
}

fn hash_of(cache: &Cache, book: i32) -> String {
    cache.checksums().all_format_records().unwrap().into_iter().find(|((id, f), _)| *id == book && f.eq_ignore_ascii_case("pdf")).map(|(_, (hash, _, _))| hash).unwrap()
}

fn options() -> PipelineOptions {
    PipelineOptions::with_workers(2)
}

#[test]
fn a_failure_is_recorded_and_not_retried_and_a_row_is_shared_by_content() {
    let (dir, cache) = library();
    let a = add_pdf(dir.path(), &cache, "one", b"%PDF-1.4 same bytes");
    let b = add_pdf(dir.path(), &cache, "two", b"%PDF-1.4 same bytes");
    let c = add_pdf(dir.path(), &cache, "three", b"%PDF-1.4 other bytes");
    let store = cache.checksums();
    assert_eq!(store.pdf_status().unwrap(), PdfStatus { fingerprinted: 0, failed: 0, pending: 3, unhashed: 0 });
    assert_eq!(store.pdf_fingerprints_pending().unwrap().len(), 3);

    // One row, written once, covers both books holding that content.
    let shared = hash_of(&cache, a);
    assert_eq!(shared, hash_of(&cache, b));
    store.record_pdf_fingerprint(&shared, &Ok(fp(1, 10, true, 0))).unwrap();
    assert!(store.has_pdf_fingerprint(&shared).unwrap());
    let pending = store.pdf_fingerprints_pending().unwrap();
    assert_eq!(pending.iter().map(|p| p.book_id).collect::<Vec<_>>(), vec![c]);
    assert_eq!(store.pdf_status().unwrap().fingerprinted, 2);

    // A failure (encrypted, corrupt) counts as done: it is not offered again.
    store.record_pdf_fingerprint(&hash_of(&cache, c), &Err("the file is encrypted".into())).unwrap();
    assert!(store.pdf_fingerprints_pending().unwrap().is_empty());
    assert_eq!(store.pdf_fingerprint_error(&hash_of(&cache, c)).unwrap().as_deref(), Some("the file is encrypted"));
    assert_eq!(store.pdf_status().unwrap(), PdfStatus { fingerprinted: 2, failed: 1, pending: 0, unhashed: 0 });

    // What is stored is what comes back, with both books on the shared row.
    let loaded = store.pdf_fingerprints().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].books, vec![a, b]);
    assert_eq!(loaded[0].fingerprint, fp(1, 10, true, 0));

    // Nothing left to do, so the job neither opens a file nor needs PDFium.
    let report = fingerprint_pdfs(&cache, &options(), &JobControl::new()).unwrap();
    assert_eq!((report.total, report.fingerprinted, report.failed_count, report.missing_count), (0, 0, 0, 0));
}

#[test]
fn a_new_version_makes_every_row_pending_again() {
    let (dir, cache) = library();
    let a = add_pdf(dir.path(), &cache, "one", b"%PDF-1.4 x");
    let hash = hash_of(&cache, a);
    cache.checksums().record_pdf_fingerprint(&hash, &Ok(fp(1, 10, false, 0))).unwrap();
    assert!(cache.checksums().pdf_fingerprints_pending().unwrap().is_empty());
    {
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("UPDATE checksums_db.pdf_fingerprints SET version = version - 1", []).unwrap();
    }
    assert_eq!(cache.checksums().pdf_fingerprints_pending().unwrap().len(), 1);
    assert!(cache.checksums().pdf_fingerprints().unwrap().is_empty(), "an old-version row is not used");
}

#[test]
fn unhashed_pdfs_are_counted_but_not_pending() {
    let (dir, cache) = library();
    let a = add_pdf(dir.path(), &cache, "one", b"%PDF-1.4 x");
    cache.checksums().remove(a, "format", "PDF").unwrap();
    assert_eq!(cache.checksums().pdf_status().unwrap(), PdfStatus { fingerprinted: 0, failed: 0, pending: 0, unhashed: 1 });
    assert!(cache.checksums().pdf_fingerprints_pending().unwrap().is_empty());
}

/// Without PDFium the job says so and records nothing: a missing library
/// must not mark every PDF as failed for ever.
#[test]
fn a_missing_pdfium_fails_the_job_without_recording_failures() {
    if calibre_ebooks::pdf::rasterize::is_available() {
        eprintln!("skipping: PDFium is installed");
        return;
    }
    let (dir, cache) = library();
    add_pdf(dir.path(), &cache, "one", b"%PDF-1.4 x");
    let error = fingerprint_pdfs(&cache, &options(), &JobControl::new()).unwrap_err();
    assert!(error.to_string().contains("PDFium"), "{error}");
    assert_eq!(cache.checksums().pdf_fingerprints_pending().unwrap().len(), 1);
}

#[test]
fn a_corrupt_pdf_is_recorded_as_failed_and_the_job_carries_on() {
    if !calibre_ebooks::pdf::rasterize::is_available() {
        eprintln!("skipping: no PDFium library available");
        return;
    }
    let (dir, cache) = library();
    let bad = add_pdf(dir.path(), &cache, "bad", b"this is not a pdf at all");
    let report = fingerprint_pdfs(&cache, &options(), &JobControl::new()).unwrap();
    assert_eq!((report.total, report.failed_count, report.fingerprinted), (1, 1, 0));
    assert!(cache.checksums().pdf_fingerprint_error(&hash_of(&cache, bad)).unwrap().is_some());
    // And a rerun has nothing to do.
    assert_eq!(fingerprint_pdfs(&cache, &options(), &JobControl::new()).unwrap().total, 0);
}

#[test]
fn a_cancelled_job_does_nothing() {
    let (dir, cache) = library();
    add_pdf(dir.path(), &cache, "one", b"%PDF-1.4 x");
    let control = JobControl::new();
    control.cancel();
    // Cancelled before it starts: no PDFium needed to say so only when there
    // is nothing to read; here there is, so it errors or reports cancelled.
    match fingerprint_pdfs(&cache, &options(), &control) {
        Ok(report) => assert!(report.cancelled && report.fingerprinted == 0),
        Err(e) => assert!(e.to_string().contains("PDFium")),
    }
    assert_eq!(cache.checksums().pdf_fingerprints_pending().unwrap().len(), 1);
}

// ---------------------------------------------------------------------
// Real PDFs (need PDFium; skipped without it)
// ---------------------------------------------------------------------

/// A PDF of one full-page gray JPEG per page, each with an invisible text
/// layer (render mode 3, which is how OCR software adds one) if `text` has
/// an entry for it.
fn pdf_of_images(jpegs: &[Vec<u8>], text: Option<&[String]>) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    let mut object = |out: &mut Vec<u8>, body: Vec<u8>| {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        out.extend_from_slice(&body);
        out.extend_from_slice(b"\nendobj\n");
    };
    let n = jpegs.len();
    // 1 catalog, 2 pages, 3 font, then per page: image, content, page.
    object(&mut out, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    let kids: String = (0..n).map(|i| format!("{} 0 R ", 6 + 3 * i)).collect();
    object(&mut out, format!("<< /Type /Pages /Kids [{kids}] /Count {n} >>").into_bytes());
    object(&mut out, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    for (i, jpeg) in jpegs.iter().enumerate() {
        let mut image = format!("<< /Type /XObject /Subtype /Image /Width {W} /Height {H} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n", jpeg.len()).into_bytes();
        image.extend_from_slice(jpeg);
        image.extend_from_slice(b"\nendstream");
        object(&mut out, image);
        let mut content = format!("q {W} 0 0 {H} 0 0 cm /Im0 Do Q\n");
        if let Some(text) = text {
            let line = text[i].replace(['(', ')', '\\'], "");
            content.push_str(&format!("BT /F1 10 Tf 3 Tr 10 300 Td ({line}) Tj ET\n"));
        }
        object(&mut out, format!("<< /Length {} >>\nstream\n{content}endstream", content.len()).into_bytes());
        object(&mut out, format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {W} {H}] /Resources << /XObject << /Im0 {} 0 R >> /Font << /F1 3 0 R >> >> /Contents {} 0 R >>", 4 + 3 * i, 5 + 3 * i).into_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for o in &offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1).as_bytes());
    out
}

fn jpeg_bytes(img: &[u8], quality: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality).encode(img, W as u32, H as u32, image::ColorType::L8).unwrap();
    bytes
}

fn page_texts(book: u64, pages: usize) -> Vec<String> {
    (0..pages).map(|p| (0..40).map(|i| format!("word{}", (book * 7919 + p as u64 * 131 + i * 17) % 4000)).collect::<Vec<_>>().join(" ")).collect()
}

/// A scan, the same scan with an OCR layer (re-encoded, shifted, with a
/// border), the OCR copy put through an "optimizer", and a different book of
/// the same length: grouped, and not. Written without a PDFium to run it
/// against in the sandbox it was developed in; it skips when PDFium is
/// absent.
#[test]
fn a_real_scan_its_ocr_copy_and_an_optimized_copy_group_and_another_book_does_not() {
    if !calibre_ebooks::pdf::rasterize::is_available() {
        eprintln!("skipping: no PDFium library available");
        return;
    }
    let pages = 12;
    let book = |seed: u64| -> Vec<Vec<u8>> { (0..pages).map(|p| page(seed * 100 + p as u64)).collect() };
    let original = book(1);
    let scan = pdf_of_images(&original.iter().enumerate().map(|(i, p)| jpeg_bytes(&noisy(p, &mut Rng(i as u64 + 1), 8.0), 75)).collect::<Vec<_>>(), None);
    let ocr_pages: Vec<Vec<u8>> = original.iter().enumerate().map(|(i, p)| scan_of(p, 500 + i as u64, 3, -2, true, 60)).collect();
    let ocr = pdf_of_images(&ocr_pages.iter().map(|p| jpeg_bytes(p, 60)).collect::<Vec<_>>(), Some(&page_texts(1, pages)));
    let optimized_pdf = pdf_of_images(&ocr_pages.iter().map(|p| jpeg_bytes(&optimized(p, 35), 35)).collect::<Vec<_>>(), Some(&page_texts(1, pages)));
    let other = pdf_of_images(&book(2).iter().map(|p| jpeg_bytes(p, 75)).collect::<Vec<_>>(), Some(&page_texts(2, pages)));

    let (dir, cache) = library();
    let ids: Vec<i32> = [("scan", &scan), ("ocr", &ocr), ("optimized", &optimized_pdf), ("other", &other)].into_iter().map(|(name, bytes)| add_pdf(dir.path(), &cache, name, bytes)).collect();

    let report = fingerprint_pdfs(&cache, &options(), &JobControl::new()).unwrap();
    assert_eq!((report.total, report.fingerprinted, report.failed_count), (4, 4, 0), "{report:?}");
    // Computed once: a second run has nothing to read.
    assert_eq!(fingerprint_pdfs(&cache, &options(), &JobControl::new()).unwrap().total, 0);

    let outcome = crate::dedupe::scan_library_full(&cache, crate::dedupe::Strength::PdfText).unwrap();
    let evidence: Vec<String> = outcome.groups.iter().flat_map(|g| g.evidence.iter().map(|e| format!("{}-{} {} {}", e.a, e.b, e.strength.reason(), e.detail))).collect();
    assert_eq!(outcome.groups.len(), 1, "{evidence:?}");
    assert_eq!(outcome.groups[0].book_ids, vec![ids[0], ids[1], ids[2]], "{evidence:?}");
    let reasons: Vec<&str> = outcome.groups[0].evidence.iter().map(|e| e.strength.reason()).collect();
    assert!(reasons.contains(&"pdf_ocr_variant") && reasons.contains(&"pdf_resave"), "{evidence:?}");
    assert!(outcome.pdf[&ids[1]].has_text && !outcome.pdf[&ids[0]].has_text);
}
