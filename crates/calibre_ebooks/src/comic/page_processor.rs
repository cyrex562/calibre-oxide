//! Port of `old_src/src/calibre/ebooks/comic/input.py`'s
//! `PageProcessor`/`render_pages`/`process_pages` (issue #124): the
//! real per-page image rendering pipeline, rewritten off
//! `QImage`/`calibre.utils.img` onto this workspace's own `image`
//! crate and already-real `calibre_utils::imageops`/`quantize`
//! (issues #569-#571).
//!
//! # Scope
//!
//! [`render_page`] ports `PageProcessor.__init__`/`render`/
//! `process_pages` step-for-step, in the exact real order. Every real
//! image operation it calls (`crop`, `rotate`, border-removal,
//! normalize, resize+pad, sharpen, despeckle, grayscale, quantize) is
//! backed by an already-real primitive from #569-#571 or a small,
//! direct port here.
//!
//! **Parallel-job orchestration is deliberately simplified**, per this
//! issue's own scope note: real `process_pages` drives a `Server`/
//! `ParallelJob` subprocess pool with progress-notification polling.
//! [`process_pages`] instead uses a bounded `std::thread::scope` +
//! shared work-queue pool, matching this crate's own established
//! pattern for "parallelize independent per-item work" (see
//! `web::feeds::download::build_index`) -- no subprocess isolation,
//! no progress callbacks, just the real per-page work itself run
//! concurrently.
//!
//! **Disclosed, not reproduced**: real `process_pages`'s final
//! `Format_Indexed8`/`Format_Grayscale16` conversion only changes how
//! Qt's own PNG *encoder* chooses a bit depth/palette for the output
//! file -- it doesn't change any pixel value beyond what
//! `quantize_image`/grayscale conversion already computed. This port
//! always writes standard 8-bit RGB(A) PNG/JPEG (matching this
//! crate's own established encoding convention elsewhere, e.g.
//! `covers.rs`), so real output files may be modestly larger than
//! Qt's own indexed/16-bit-grayscale encoding would produce, but pixel
//! *values* are unaffected.
//!
//! **Disclosed, narrower mechanism, same practical outcome**: real
//! Python detects "was the source image already grayscale" from Qt's
//! own image FORMAT metadata (`Format_Grayscale8`/`_16`, or
//! `Format_Indexed8` with `img.allGray()`). This port instead checks
//! the actual pixel content (every pixel has `r == g == b`) --
//! `image::load_from_memory` always decodes to a uniform RGBA8 buffer
//! structurally, so there is no analogous format-metadata to read.
//! Any real grayscale scan produces the same detection result either
//! way; the only difference is mechanism, not user-visible outcome.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use image::{Rgba, RgbaImage};

use calibre_utils::imageops::{despeckle, gaussian_sharpen, grayscale, normalize, overlay, remove_borders};
use calibre_utils::quantize::quantize;

use crate::oeb::transforms::rescale::fit_image;
use crate::web::fetch::utils::encode_jpeg;

/// Real Python's own `MAX_SCREEN_SIZE` constant: if a computed resize
/// target would exceed this in either dimension, the resize (and its
/// accompanying border-pad) is skipped entirely, leaving the image at
/// its current size -- upstream's own guard against a pathological
/// resize failure for absurdly large screen-size requests.
const MAX_SCREEN_SIZE: u32 = 3000;

/// Port of the real `ComicInput` conversion plugin's own
/// `OptionRecommendation`s that `PageProcessor` reads, real defaults
/// preserved (`colors=0` meaning "off", `output_format="png"`, every
/// boolean `false`, `comic_screen_size` from the base
/// `OutputProfile.comic_screen_size = (584, 754)`).
#[derive(Debug, Clone)]
pub struct ComicPageOptions {
    pub landscape: bool,
    pub right2left: bool,
    pub disable_trim: bool,
    pub dont_normalize: bool,
    pub keep_aspect_ratio: bool,
    pub wide: bool,
    pub dont_sharpen: bool,
    pub despeckle: bool,
    pub dont_grayscale: bool,
    /// `"png"` or `"jpg"`, matching real Python's own
    /// `output_format` choices.
    pub output_format: String,
    /// `0` means "off" (real Python's own sentinel), matching
    /// `colors: recommended_value=0`.
    pub colors: u32,
    /// Overrides `comic_screen_size` when set -- real Python's own
    /// `comic_image_size` (a `"WxH"` string there, already parsed
    /// here).
    pub comic_image_size: Option<(u32, u32)>,
    pub comic_screen_size: (u32, u32),
    pub verbose: bool,
}

impl Default for ComicPageOptions {
    fn default() -> Self {
        ComicPageOptions {
            landscape: false,
            right2left: false,
            disable_trim: false,
            dont_normalize: false,
            keep_aspect_ratio: false,
            wide: false,
            dont_sharpen: false,
            despeckle: false,
            dont_grayscale: false,
            output_format: "png".to_string(),
            colors: 0,
            comic_image_size: None,
            comic_screen_size: (584, 754),
            verbose: false,
        }
    }
}

fn is_grayscale(img: &RgbaImage) -> bool {
    img.pixels().all(|p| p[0] == p[1] && p[1] == p[2])
}

/// Port of `crop_image`: clamps `width`/`height` to the source's own
/// remaining extent past `(x, y)`, matching real Python's own
/// `width = min(width, img.width() - x)` guard.
fn crop_image(img: &RgbaImage, x: u32, y: u32, width: u32, height: u32) -> RgbaImage {
    let (iw, ih) = img.dimensions();
    let w = width.min(iw.saturating_sub(x));
    let h = height.min(ih.saturating_sub(y));
    image::imageops::crop_imm(img, x, y, w, h).to_image()
}

/// Port of `add_borders_to_image` (`calibre.utils.img`), operating
/// directly on [`RgbaImage`] via the already-real [`overlay`] rather
/// than round-tripping through encoded bytes the way
/// `web::feeds::cover::add_borders_to_image` does for its own
/// (byte-in, byte-out) real call site.
fn add_borders(img: &RgbaImage, left: u32, top: u32, right: u32, bottom: u32) -> RgbaImage {
    if left == 0 && top == 0 && right == 0 && bottom == 0 {
        return img.clone();
    }
    let (w, h) = img.dimensions();
    let mut canvas = RgbaImage::from_pixel(w + left + right, h + top + bottom, Rgba([255, 255, 255, 255]));
    let _ = overlay(&mut canvas, img, left, top);
    canvas
}

/// Port of `resize_image`: an aspect-ratio-*ignoring* resize (real
/// Python's own `Qt::AspectRatioMode.IgnoreAspectRatio` +
/// `SmoothTransformation`), matching this crate's own established
/// Lanczos3 convention for "smooth" scaling elsewhere (`web/fetch/utils.rs`).
fn resize_image(img: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    image::imageops::resize(img, width.max(1), height.max(1), image::imageops::FilterType::Lanczos3)
}

/// Port of `scale_image(img, as_png=True)`'s own real defaults
/// (`width=60, height=80, preserve_aspect_ratio=True`), used for the
/// first page's thumbnail.
fn scale_to_thumbnail_png(img: &RgbaImage) -> Vec<u8> {
    let (w, h) = img.dimensions();
    let (scaled, nw, nh) = fit_image(w as f64, h as f64, 60.0, 80.0);
    let out = if scaled { resize_image(img, nw.max(1) as u32, nh.max(1) as u32) } else { img.clone() };
    let mut buf = Vec::new();
    let _ = image::DynamicImage::ImageRgba8(out).write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png);
    buf
}

fn encode_page(img: &RgbaImage, output_format: &str) -> Vec<u8> {
    if output_format.eq_ignore_ascii_case("png") {
        let mut buf = Vec::new();
        let _ = image::DynamicImage::ImageRgba8(img.clone()).write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png);
        buf
    } else {
        encode_jpeg(&image::DynamicImage::ImageRgba8(img.clone()), 90)
    }
}

/// Port of `PageProcessor.process_pages`'s own fit-and-pad branch,
/// shared by the real `keep_aspect_ratio` and `wide` cases (which
/// differ only in the effective screen box they fit into -- see
/// [`render_page`]'s own call sites).
fn fit_and_pad(page: RgbaImage, scr_width: u32, scr_height: u32) -> RgbaImage {
    let (sizex, sizey) = page.dimensions();
    let aspect = sizex as f64 / sizey as f64;
    let screen_aspect = scr_width as f64 / scr_height as f64;

    let (newsizex, newsizey, deltax, deltay) = if aspect <= screen_aspect {
        let newsizey = scr_height;
        let newsizex = (newsizey as f64 * aspect) as u32;
        let deltax = (scr_width.saturating_sub(newsizex)) / 2;
        (newsizex, newsizey, deltax, 0u32)
    } else {
        let newsizex = scr_width;
        let newsizey = (newsizex as f64 / aspect).floor() as u32;
        let deltay = (scr_height.saturating_sub(newsizey)) / 2;
        (newsizex, newsizey, 0u32, deltay)
    };

    if newsizex < MAX_SCREEN_SIZE && newsizey < MAX_SCREEN_SIZE {
        let resized = resize_image(&page, newsizex.max(1), newsizey.max(1));
        add_borders(&resized, deltax, deltay, deltax, deltay)
    } else {
        // Real Python's own comment: "Too large and resizing fails,
        // so better to leave it as original size."
        page
    }
}

/// Port of `PageProcessor.__init__`/`render`/`process_pages`: renders
/// one source page (splitting a landscape page into two portrait
/// pages unless `landscape`/`right2left` say otherwise) into one or
/// more output image files under `dest_dir`, returning their paths in
/// real Python's own `self.append(dest)` order.
pub fn render_page(path_to_page: &Path, dest_dir: &Path, opts: &ComicPageOptions, num: usize) -> Result<Vec<PathBuf>> {
    let raw = std::fs::read(path_to_page).with_context(|| format!("reading {}", path_to_page.display()))?;
    let src = image::load_from_memory(&raw).with_context(|| format!("decoding {}", path_to_page.display()))?.to_rgba8();
    let (width, height) = src.dimensions();

    if num == 0 {
        std::fs::write(dest_dir.join("thumbnail.png"), scale_to_thumbnail_png(&src))?;
    }

    let src_was_grayscale = is_grayscale(&src);

    let mut rotate = false;
    let pages: Vec<RgbaImage> = if width > height {
        if opts.landscape {
            rotate = true;
            vec![src]
        } else {
            let half = width / 2;
            let split1 = crop_image(&src, 0, 0, half, height);
            let split2 = crop_image(&src, half, 0, width - half, height);
            if opts.right2left { vec![split2, split1] } else { vec![split1, split2] }
        }
    } else {
        vec![src]
    };

    let mut outputs = Vec::with_capacity(pages.len());
    for (i, mut page) in pages.into_iter().enumerate() {
        if rotate {
            // Port of `rotate_image(img, -90)`: Qt's `QTransform::rotate`
            // takes positive angles as clockwise, so -90 is 90
            // counter-clockwise, matching `image::imageops::rotate270`
            // (270 clockwise ≡ 90 counter-clockwise).
            page = image::imageops::rotate270(&page);
        }

        if !opts.disable_trim {
            // Real Python's own default fuzz value (`tweaks['cover_trim_fuzz_value']`).
            page = remove_borders(&page, 10.0);
        }
        if !opts.dont_normalize {
            page = normalize(&page);
        }

        let (scr_width, scr_height) = opts.comic_image_size.unwrap_or(opts.comic_screen_size);

        if opts.keep_aspect_ratio {
            page = fit_and_pad(page, scr_width, scr_height);
        } else if opts.wide {
            // Real Python: use device height as the landscape-mode
            // screen width, +25px back for the battery bar.
            let wscreenx = scr_height + 25;
            let screen_aspect = scr_width as f64 / scr_height as f64;
            let wscreeny = (wscreenx as f64 / screen_aspect).floor() as u32;
            page = fit_and_pad(page, wscreenx, wscreeny.max(1));
        } else if scr_width < MAX_SCREEN_SIZE && scr_height < MAX_SCREEN_SIZE {
            page = resize_image(&page, scr_width, scr_height);
        }

        if !opts.dont_sharpen {
            page = gaussian_sharpen(&page, 0.0, 1.0, true);
        }
        if opts.despeckle {
            page = despeckle(&page);
        }

        if !opts.dont_grayscale {
            page = grayscale(&page);
        }

        if opts.output_format.eq_ignore_ascii_case("png") && opts.colors > 0 {
            page = quantize(&page, opts.colors.min(256), true);
        }

        let dest_path = dest_dir.join(format!("{num}_{i}.{}", opts.output_format));
        std::fs::write(&dest_path, encode_page(&page, &opts.output_format))?;
        outputs.push(dest_path);
    }

    let _ = src_was_grayscale; // real Python threads this through only to the now-disclosed-narrowed encoding bit-depth choice
    Ok(outputs)
}

/// One rendering task -- a `(page_index, source_path)` pair, port of
/// real Python's own `(num, path)` tuples.
pub type PageTask = (usize, PathBuf);

/// The outcome of rendering a batch of pages -- port of real
/// `render_pages`'/`process_pages`'s own `(pages, failures)` tuple
/// return shape.
#[derive(Debug, Default)]
pub struct RenderOutcome {
    pub pages: Vec<PathBuf>,
    pub failures: Vec<PathBuf>,
}

/// Port of `render_pages`: renders a batch of tasks sequentially,
/// recording per-page failures rather than aborting the whole batch
/// (matching real Python's own per-page `try/except`).
pub fn render_pages(tasks: &[PageTask], dest_dir: &Path, opts: &ComicPageOptions) -> RenderOutcome {
    let mut outcome = RenderOutcome::default();
    for (num, path) in tasks {
        match render_page(path, dest_dir, opts, *num) {
            Ok(mut out) => outcome.pages.append(&mut out),
            Err(e) => {
                if opts.verbose {
                    eprintln!("Failed {}: {e:#}", path.display());
                }
                outcome.failures.push(path.clone());
            }
        }
    }
    outcome
}

/// Port of the module-level `process_pages`: renders every page
/// concurrently via a bounded worker pool (`std::thread::scope` +
/// shared queue, matching this crate's own established pattern for
/// this shape of work -- see this module's own doc for why the real
/// `Server`/`ParallelJob` subprocess orchestration isn't reproduced).
pub fn process_pages(pages: &[PathBuf], opts: &ComicPageOptions, dest_dir: &Path, num_workers: usize) -> RenderOutcome {
    let tasks: Vec<PageTask> = pages.iter().cloned().enumerate().collect();
    let queue = Mutex::new(tasks.into_iter());
    let num_workers = num_workers.max(1);

    let results: Vec<RenderOutcome> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..num_workers)
            .map(|_| {
                let queue = &queue;
                scope.spawn(move || {
                    let mut local = RenderOutcome::default();
                    loop {
                        let task = { queue.lock().unwrap().next() };
                        let Some((num, path)) = task else { break };
                        match render_page(&path, dest_dir, opts, num) {
                            Ok(mut out) => local.pages.append(&mut out),
                            Err(_) => local.failures.push(path),
                        }
                    }
                    local
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or_default()).collect()
    });

    let mut outcome = RenderOutcome::default();
    for mut r in results {
        outcome.pages.append(&mut r.pages);
        outcome.failures.append(&mut r.failures);
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;

    fn write_png(path: &Path, w: u32, h: u32, color: [u8; 4]) {
        let img = RgbaImage::from_pixel(w, h, Rgba(color));
        image::DynamicImage::ImageRgba8(img).save(path).unwrap();
    }

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("calibre-oxide-test-comic-page-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn render_page_writes_a_thumbnail_only_for_the_first_page() {
        let dir = test_dir("thumbnail");
        let src = dir.join("page.png");
        write_png(&src, 100, 140, [10, 20, 30, 255]);
        let opts = ComicPageOptions::default();

        render_page(&src, &dir, &opts, 0).unwrap();
        assert!(dir.join("thumbnail.png").is_file());

        let src2 = dir.join("page2.png");
        write_png(&src2, 100, 140, [10, 20, 30, 255]);
        std::fs::remove_file(dir.join("thumbnail.png")).unwrap();
        render_page(&src2, &dir, &opts, 1).unwrap();
        assert!(!dir.join("thumbnail.png").is_file(), "only page 0 gets a thumbnail");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn render_page_splits_a_landscape_page_into_two_portrait_pages() {
        let dir = test_dir("split");
        let src = dir.join("wide.png");
        write_png(&src, 200, 100, [50, 50, 50, 255]);
        let opts = ComicPageOptions { dont_sharpen: true, dont_normalize: true, disable_trim: true, dont_grayscale: true, ..Default::default() };

        let outputs = render_page(&src, &dir, &opts, 0).unwrap();
        assert_eq!(outputs.len(), 2, "a landscape page not marked `landscape` should split into two pages");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn render_page_keeps_a_landscape_page_whole_when_landscape_option_is_set() {
        let dir = test_dir("no-split");
        let src = dir.join("wide.png");
        write_png(&src, 200, 100, [50, 50, 50, 255]);
        let opts = ComicPageOptions { landscape: true, dont_sharpen: true, dont_normalize: true, disable_trim: true, dont_grayscale: true, ..Default::default() };

        let outputs = render_page(&src, &dir, &opts, 0).unwrap();
        assert_eq!(outputs.len(), 1, "landscape=true should keep the page whole (and rotate it) instead of splitting");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn render_page_right2left_swaps_split_order() {
        let dir = test_dir("r2l");
        let src = dir.join("wide.png");
        // Left half red, right half blue, so we can tell which split
        // ends up first in the output filenames.
        let mut img = RgbaImage::new(200, 100);
        for y in 0..100 {
            for x in 0..200 {
                let color = if x < 100 { [255, 0, 0, 255] } else { [0, 0, 255, 255] };
                img.put_pixel(x, y, Rgba(color));
            }
        }
        image::DynamicImage::ImageRgba8(img).save(&src).unwrap();

        let base_opts = ComicPageOptions { dont_sharpen: true, dont_normalize: true, disable_trim: true, dont_grayscale: true, output_format: "png".to_string(), ..Default::default() };

        let ltr = render_page(&src, &dir, &base_opts, 0).unwrap();
        let ltr_first = image::open(&ltr[0]).unwrap().to_rgba8();

        let r2l_opts = ComicPageOptions { right2left: true, ..base_opts };
        let r2l = render_page(&src, &dir, &r2l_opts, 1).unwrap();
        let r2l_first = image::open(&r2l[0]).unwrap().to_rgba8();

        assert_ne!(ltr_first.get_pixel(0, 0), r2l_first.get_pixel(0, 0), "right2left should reverse which split comes first");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn render_page_grayscale_by_default() {
        let dir = test_dir("grayscale");
        let src = dir.join("color.png");
        write_png(&src, 50, 60, [200, 50, 50, 255]);
        let opts = ComicPageOptions { dont_sharpen: true, dont_normalize: true, disable_trim: true, ..Default::default() };

        let outputs = render_page(&src, &dir, &opts, 0).unwrap();
        let out_img = image::open(&outputs[0]).unwrap().to_rgba8();
        let p = out_img.get_pixel(25, 30);
        assert_eq!(p[0], p[1], "default behavior should grayscale the output");
        assert_eq!(p[1], p[2]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn render_page_dont_grayscale_preserves_color() {
        let dir = test_dir("keep-color");
        let src = dir.join("color.png");
        write_png(&src, 50, 60, [200, 50, 50, 255]);
        let opts = ComicPageOptions { dont_grayscale: true, dont_sharpen: true, dont_normalize: true, disable_trim: true, ..Default::default() };

        let outputs = render_page(&src, &dir, &opts, 0).unwrap();
        let out_img = image::open(&outputs[0]).unwrap().to_rgba8();
        let p = out_img.get_pixel(25, 30);
        assert!(p[0] > p[1], "dont_grayscale should preserve the real color channel imbalance");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn render_page_resizes_to_the_configured_screen_size_by_default() {
        let dir = test_dir("resize");
        let src = dir.join("page.png");
        write_png(&src, 100, 100, [10, 10, 10, 255]); // square, non-landscape
        let opts = ComicPageOptions { dont_sharpen: true, dont_normalize: true, disable_trim: true, comic_screen_size: (200, 300), ..Default::default() };

        let outputs = render_page(&src, &dir, &opts, 0).unwrap();
        let out_img = image::open(&outputs[0]).unwrap();
        assert_eq!(out_img.dimensions(), (200, 300), "the default (non-keep-aspect, non-wide) path should ignore aspect ratio and fill the screen exactly");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn render_page_keep_aspect_ratio_pads_instead_of_stretching() {
        let dir = test_dir("keep-aspect");
        let src = dir.join("page.png");
        write_png(&src, 100, 100, [10, 10, 10, 255]);
        let opts = ComicPageOptions { keep_aspect_ratio: true, dont_sharpen: true, dont_normalize: true, disable_trim: true, comic_screen_size: (200, 400), ..Default::default() };

        let outputs = render_page(&src, &dir, &opts, 0).unwrap();
        let out_img = image::open(&outputs[0]).unwrap();
        // The output canvas should still exactly match the screen
        // size (padding fills the gap), even though a square source
        // doesn't fill a 200x400 box directly.
        assert_eq!(out_img.dimensions(), (200, 400));
        // The corners should be white padding.
        let rgba = out_img.to_rgba8();
        assert_eq!(rgba.get_pixel(0, 0).0, [255, 255, 255, 255]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn process_pages_renders_every_page_concurrently_and_reports_failures() {
        let dir = test_dir("process-pages");
        let ok1 = dir.join("a.png");
        let ok2 = dir.join("b.png");
        write_png(&ok1, 40, 50, [1, 2, 3, 255]);
        write_png(&ok2, 40, 50, [4, 5, 6, 255]);
        let bad = dir.join("not_an_image.png");
        std::fs::write(&bad, b"not a real image").unwrap();

        let opts = ComicPageOptions { dont_sharpen: true, dont_normalize: true, disable_trim: true, ..Default::default() };
        let outcome = process_pages(&[ok1, bad.clone(), ok2], &opts, &dir, 2);

        assert_eq!(outcome.pages.len(), 2, "the two real images should each produce one output page");
        assert_eq!(outcome.failures, vec![bad]);

        std::fs::remove_dir_all(&dir).ok();
    }
}
