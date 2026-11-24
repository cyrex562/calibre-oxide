//! The evidence behind `pdf_similarity::PAGE_MATCH_DISTANCE`, measured on real
//! PDFium renders of real typeset text (unlike the unit tests, whose pages are
//! synthetic blocks).
//!
//! Two 24-page books in Times-Roman are generated from seeded pseudo-text and
//! rendered; their pages are then put through a crude scanner (paper tone,
//! noise, a dark border strip, rotation, JPEG) and an "optimizer" (downsample,
//! hard JPEG). It reports the signature distance of each page to its own copy
//! and to every other page, and asserts the targets the threshold was set by:
//!
//! * every clean and scanned page gets a signature;
//! * at least 95% of same-page pairs are within the threshold;
//! * at most 1% of different-page pairs are.
//!
//! A rotated *independent* scan (0.4 and 0.8 degrees) is reported but not
//! asserted: the signature does not survive it, and two scans of one book on
//! different scanners are found by their OCR text instead, not their pixels.
//!
//! Ignored because it renders ~150 pages; it skips when PDFium is missing.
//!
//! ```text
//! LEXMECHANIC_PDFIUM=<dir with pdfium.dll> cargo test -p lexmechanic_db --test pdf_signature_benchmark -- --ignored --nocapture
//! ```

use lexmechanic_db::pdf_similarity::{page_distance, page_signature, PageHash, PAGE_MATCH_DISTANCE, RENDER_LONG_SIDE, SIGNATURE_BITS};
use lexmechanic_ebooks::pdf::rasterize::{is_available, sample_pdf_from_path};

const PAGES: usize = 24;
const LINES_PER_PAGE: usize = 46;
const CHARS_PER_LINE: usize = 82;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A pseudo-English word: one to four syllables.
fn word(r: &mut Rng) -> String {
    const SYLLABLES: &[&str] = &["the", "an", "con", "ver", "sa", "tion", "ing", "er", "o", "mi", "tra", "pel", "ent", "is", "ra", "du", "ly", "for", "ab", "le", "re", "ma", "ti", "on", "a", "st", "ri", "ke", "wo", "nd", "gh", "ou"];
    let n = 1 + r.below(4) as usize;
    let mut w: String = (0..n).map(|_| SYLLABLES[r.below(SYLLABLES.len() as u64) as usize]).collect();
    if r.below(14) == 0 {
        let mut c = w.chars();
        w = c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default();
    }
    match r.below(12) {
        0 => w.push(','),
        1 => w.push('.'),
        _ => {}
    }
    w
}

/// `PAGES` pages of lines; a line starting with `#` is a chapter heading.
fn book(seed: u64) -> Vec<Vec<String>> {
    let mut r = Rng(seed);
    let mut pages: Vec<Vec<String>> = vec![Vec::new()];
    let mut chapter = 1;
    loop {
        // A paragraph, or (now and then, at a page's start) a chapter heading.
        if pages.last().unwrap().is_empty() && r.below(5) == 0 {
            pages.last_mut().unwrap().push(format!("#Chapter {chapter}"));
            pages.last_mut().unwrap().push(String::new());
            chapter += 1;
        }
        let words = 15 + r.below(110);
        let mut line = String::from("    ");
        for _ in 0..words {
            let w = word(&mut r);
            if line.len() + w.len() + 1 > CHARS_PER_LINE {
                pages.last_mut().unwrap().push(std::mem::take(&mut line));
                if pages.last().unwrap().len() >= LINES_PER_PAGE {
                    if pages.len() == PAGES {
                        return pages;
                    }
                    pages.push(Vec::new());
                }
            }
            if !line.is_empty() && !line.ends_with(' ') {
                line.push(' ');
            }
            line.push_str(&w);
        }
        pages.last_mut().unwrap().push(line);
        if pages.last().unwrap().len() >= LINES_PER_PAGE {
            if pages.len() == PAGES {
                return pages;
            }
            pages.push(Vec::new());
        }
    }
}

/// A plain PDF of `pages`: A4, Times-Roman 11pt, bold 20pt headings, page numbers.
fn pdf(pages: &[Vec<String>]) -> Vec<u8> {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)");
    let mut objs: Vec<String> = Vec::new();
    let n = pages.len();
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 5 + 2 * i)).collect();
    objs.push("<< /Type /Catalog /Pages 2 0 R >>".into());
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman >>".into());
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Times-Bold >>".into());
    for (i, lines) in pages.iter().enumerate() {
        let mut s = String::from("BT\n");
        let mut y = 760.0;
        for l in lines {
            if let Some(h) = l.strip_prefix('#') {
                s += &format!("/F2 20 Tf 1 0 0 1 72 {y} Tm ({}) Tj\n", esc(h));
                y -= 30.0;
            } else {
                s += &format!("/F1 11 Tf 1 0 0 1 72 {y} Tm ({}) Tj\n", esc(l));
                y -= 14.0;
            }
        }
        s += &format!("/F1 10 Tf 1 0 0 1 290 40 Tm ({}) Tj\nET\n", i + 1);
        objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {} 0 R >>", 6 + 2 * i));
        objs.push(format!("<< /Length {} >>\nstream\n{s}endstream", s.len()));
    }
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

struct Gray {
    px: Vec<u8>,
    w: u32,
    h: u32,
}

fn signature(g: &Gray) -> Option<PageHash> {
    page_signature(&g.px, g.w, g.h).map(|(bits, mask)| PageHash { index: 0, bits, mask, blank: false })
}

fn distance(a: &PageHash, b: &PageHash) -> u32 {
    page_distance(a, b).unwrap_or(SIGNATURE_BITS)
}

/// Resized so its long side is `long`, as the fingerprint job renders.
fn shrink(g: &Gray, long: f32) -> Gray {
    let img = image::GrayImage::from_raw(g.w, g.h, g.px.clone()).unwrap();
    let scale = long / g.w.max(g.h) as f32;
    let (w, h) = ((g.w as f32 * scale).round() as u32, (g.h as f32 * scale).round() as u32);
    Gray { px: image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle).into_raw(), w, h }
}

fn jpeg(g: &Gray, quality: u8) -> Gray {
    let img = image::GrayImage::from_raw(g.w, g.h, g.px.clone()).unwrap();
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality).encode_image(&img).unwrap();
    Gray { px: image::load_from_memory(&bytes).unwrap().to_luma8().into_raw(), w: g.w, h: g.h }
}

/// A crude scanner: rotation, a dark border strip on two edges, paper tone,
/// noise, JPEG.
fn scan(g: &Gray, degrees: f32, seed: u64, quality: u8) -> Gray {
    let (s, c) = degrees.to_radians().sin_cos();
    let (cx, cy) = (g.w as f32 / 2.0, g.h as f32 / 2.0);
    let mut r = Rng(seed);
    let mut px = vec![255u8; (g.w * g.h) as usize];
    for y in 0..g.h {
        for x in 0..g.w {
            let (dx, dy) = (x as f32 - cx, y as f32 - cy);
            let (sx, sy) = (c * dx + s * dy + cx, -s * dx + c * dy + cy);
            let mut v = if sx >= 0.0 && sy >= 0.0 && (sx as u32) < g.w && (sy as u32) < g.h { g.px[(sy as u32 * g.w + sx as u32) as usize] as f32 } else { 30.0 };
            if x < g.w / 40 || y < g.h / 60 {
                v = 25.0;
            }
            let noise = r.below(41) as f32 - 20.0;
            px[(y * g.w + x) as usize] = (v * 0.92 + 8.0 + noise).clamp(0.0, 255.0) as u8;
        }
    }
    jpeg(&Gray { px, w: g.w, h: g.h }, quality)
}

struct Dist(Vec<u32>);
impl Dist {
    fn report(&mut self, name: &str) {
        self.0.sort_unstable();
        let within = self.within();
        let v = &self.0;
        let p = |q: f32| v[((v.len() - 1) as f32 * q) as usize];
        println!("  {name}: n={} min={} p5={} p50={} p95={} max={} within {PAGE_MATCH_DISTANCE}: {:.1}%", v.len(), v[0], p(0.05), p(0.5), p(0.95), v[v.len() - 1], 100.0 * within);
    }
    fn within(&self) -> f32 {
        self.0.iter().filter(|&&d| d <= PAGE_MATCH_DISTANCE).count() as f32 / self.0.len() as f32
    }
}

/// Distances from each of `copies` to the same index of `originals`, and to
/// every other index.
fn compare(originals: &[PageHash], copies: &[PageHash]) -> (Dist, Dist) {
    let (mut same, mut other) = (Vec::new(), Vec::new());
    for (i, c) in copies.iter().enumerate() {
        for (j, o) in originals.iter().enumerate() {
            if i == j { same.push(distance(o, c)) } else { other.push(distance(o, c)) }
        }
    }
    (Dist(same), Dist(other))
}

fn all_signed(pages: &[Gray], what: &str) -> Vec<PageHash> {
    let sigs: Vec<PageHash> = pages.iter().filter_map(signature).collect();
    println!("  {what}: {}/{} pages signed", sigs.len(), pages.len());
    assert_eq!(sigs.len(), pages.len(), "{what}: a text page got no signature");
    sigs
}

#[test]
#[ignore]
fn page_signatures_on_real_renders() {
    if !is_available() {
        eprintln!("skipping: no PDFium library available");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let long = RENDER_LONG_SIDE as f32;
    let render = |seed: u64, side: u32| -> Vec<Gray> {
        let path = dir.path().join(format!("book{seed}.pdf"));
        if !path.exists() {
            std::fs::write(&path, pdf(&book(seed))).unwrap();
        }
        let all = |n: usize| ((0..n).collect::<Vec<_>>(), Vec::new());
        let sampled = sample_pdf_from_path(&path, side, all).unwrap();
        assert_eq!(sampled.pages.len(), PAGES);
        sampled.pages.into_iter().map(|p| Gray { px: p.gray, w: p.width, h: p.height }).collect()
    };
    // What the fingerprint job sees, and a scanner-resolution render to scan.
    let (a, b) = (render(1, RENDER_LONG_SIDE), render(2, RENDER_LONG_SIDE));
    let a_hi = render(1, 1400);

    println!("clean renders");
    let sa = all_signed(&a, "book A");
    let sb = all_signed(&b, "book B");
    let mut within_book = Dist(sa.iter().enumerate().flat_map(|(i, x)| sa[i + 1..].iter().map(move |y| distance(x, y))).collect());
    within_book.report("different pages, same book");
    let mut across = Dist(sa.iter().flat_map(|x| sb.iter().map(move |y| distance(x, y))).collect());
    across.report("book A against book B");
    assert!(within_book.within() <= 0.01 && across.within() <= 0.01);

    for (degrees, quality, asserted) in [(0.0f32, 75u8, true), (0.4, 60, false), (0.8, 45, false)] {
        println!("independent scan, {degrees} degrees, JPEG q{quality}{}", if asserted { "" } else { " (reported only)" });
        let scans: Vec<Gray> = a_hi.iter().enumerate().map(|(i, p)| shrink(&scan(p, degrees, i as u64 + 7, quality), long)).collect();
        let ss = all_signed(&scans, "scan");
        let (mut same, mut other) = compare(&sa, &ss);
        same.report("same page");
        other.report("other pages");
        if asserted {
            assert!(same.within() >= 0.95 && other.within() <= 0.01);
        }
    }

    // One scan (slightly skewed, at scanner resolution), and what optimizers
    // and re-savers do to it.
    let base: Vec<Gray> = a_hi.iter().enumerate().map(|(i, p)| scan(p, 0.6, i as u64 + 3, 90)).collect();
    let base_sigs = all_signed(&base.iter().map(|g| shrink(g, long)).collect::<Vec<_>>(), "base scan");
    for (quality, scale) in [(85u8, 1.0f32), (50, 1.0), (30, 0.5), (20, 0.35)] {
        println!("the same scan re-encoded at q{quality}, x{scale}");
        let copies: Vec<Gray> = base
            .iter()
            .map(|g| {
                let small = shrink(g, g.w.max(g.h) as f32 * scale);
                shrink(&jpeg(&small, quality), long)
            })
            .collect();
        let cs = all_signed(&copies, "copy");
        let (mut same, mut other) = compare(&base_sigs, &cs);
        same.report("same page");
        other.report("other pages");
        assert!(same.within() >= 0.95 && other.within() <= 0.01);
    }
}
