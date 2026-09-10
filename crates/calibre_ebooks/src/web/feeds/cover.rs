//! Port of `BasicNewsRecipe`'s cover/masthead download-and-generate
//! glue (`_download_cover`/`download_cover`/`_download_masthead`/
//! `download_masthead`/`resolve_masthead`/`default_cover`/
//! `default_masthead_image`/`prepare_masthead_image`,
//! `old_src/src/calibre/web/feeds/news.py`, issue #621, split from
//! #81), plus the narrow slice of `calibre.utils.img`
//! (`add_borders_to_image`/`image_to_data`/`save_cover_data_to`) that
//! `_download_cover`'s border-compositing branch needs -- none of
//! which existed anywhere in this crate yet (confirmed via grep
//! before writing this module).
//!
//! Real image fetch goes through the already-real
//! [`crate::scraper::Browser`] (#58); the masthead-fit and default-
//! cover paths reuse the already-real
//! [`crate::web::fetch::utils::prepare_masthead_image`] and
//! [`crate::covers::{create_cover, generate_masthead}`] respectively.
//!
//! # Scope
//!
//! **Disclosed narrowing**: real `_download_cover` has a PDF-specific
//! branch (`ext == 'pdf'`) that extracts a cover from PDF metadata via
//! `calibre.ebooks.metadata.pdf.get_metadata`. No PDF metadata/cover-
//! extraction path exists anywhere in this crate's `pdf` module yet
//! (checked: `develop.rs`/`html_writer.rs`/`pdftohtml.rs`/`reflow.rs`/
//! `utils.rs`/`image_writer.rs`/`render/` -- no `metadata.rs`).
//! [`download_cover`] treats a fetched PDF-extension cover URL as "no
//! cover data extracted" -- the same observable outcome as real
//! Python's own `get_metadata` returning no `cover_data`, just via a
//! narrower mechanism. Revisit once PDF metadata extraction exists.
//!
//! **Disclosed narrowing, falls out of #619's own hook shape**: real
//! `get_cover_url`/`get_masthead_url` can return an already-open,
//! readable file-like object (`hasattr(cu, 'read')`) rather than a
//! path or URL string. [`crate::web::feeds::recipe::NewsRecipeHooks`]'s
//! hooks return `Option<String>` (established in #619, not narrowed
//! further here), so that branch has no reachable equivalent in this
//! port; the local-path and remote-URL branches are both real.
//!
//! `save_cover_data_to`'s general-purpose resize/grayscale/letterbox
//! options are not ported -- this module's own real call site
//! (`cpath = output_dir.join("cover.jpg")`, no extra options) never
//! uses them, matching the same scoped-reimplementation discipline
//! `web/fetch/utils.rs` already applies to the rest of `utils/img.py`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use image::{DynamicImage, GenericImageView};

use crate::covers::{create_cover, generate_masthead, CoverPrefs};
use crate::oeb::color3::{parse_color_string, CssColor, Rgba};
use crate::scraper::{Browser, OpenOptions};
use crate::web::fetch::utils::{encode_jpeg, prepare_masthead_image};

use super::recipe::NewsRecipeHooks;

/// Port of `BasicNewsRecipe.MI_WIDTH`.
pub const MI_WIDTH: u32 = 600;
/// Port of `BasicNewsRecipe.MI_HEIGHT`.
pub const MI_HEIGHT: u32 = 60;

// ===================================================================
// The narrow `calibre.utils.img` slice this module needs
// ===================================================================

fn resolve_border_color(spec: &str) -> Rgba {
    match parse_color_string(spec) {
        Some(CssColor::Rgba(rgba)) => rgba,
        _ => Rgba { red: 1.0, green: 1.0, blue: 1.0, alpha: 1.0 },
    }
}

/// Port of `add_borders_to_image`. Returns `img_data` decoded and
/// padded with a solid-color border on each side; if all four margins
/// are zero, returns the image unchanged (matching real Python's own
/// early-return).
pub fn add_borders_to_image(img_data: &[u8], left: u32, top: u32, right: u32, bottom: u32, border_color: &str) -> anyhow::Result<DynamicImage> {
    let img = image::load_from_memory(img_data)?;
    if left == 0 && top == 0 && right == 0 && bottom == 0 {
        return Ok(img);
    }
    let rgba = resolve_border_color(border_color);
    let fill = image::Rgba([
        (rgba.red * 255.0).round() as u8,
        (rgba.green * 255.0).round() as u8,
        (rgba.blue * 255.0).round() as u8,
        255,
    ]);
    let (w, h) = img.dimensions();
    let mut canvas = DynamicImage::new_rgba8(w + left + right, h + top + bottom);
    if let Some(buf) = canvas.as_mut_rgba8() {
        for pixel in buf.pixels_mut() {
            *pixel = fill;
        }
    }
    image::imageops::overlay(&mut canvas, &img, left as i64, top as i64);
    Ok(canvas)
}

/// Port of `image_to_data`, restricted to its own real default form
/// (`fmt='JPEG'`) -- the only form any real call site in this cluster
/// uses.
pub fn image_to_data(img: &DynamicImage, quality: u8) -> Vec<u8> {
    encode_jpeg(img, quality)
}

/// Port of `save_cover_data_to`'s real call-site shape: decode
/// `data`, flatten any transparency onto white, and write it to
/// `path` as a JPEG.
pub fn save_cover_data_to(data: &[u8], path: &Path) -> anyhow::Result<()> {
    let img = image::load_from_memory(data)?;
    let encoded = encode_jpeg(&img, 90);
    std::fs::write(path, encoded)?;
    Ok(())
}

// ===================================================================
// Fetching (local file or remote URL)
// ===================================================================

fn fetch_bytes(browser: &Browser, timeout: Duration, url_or_path: &str) -> anyhow::Result<Vec<u8>> {
    if Path::new(url_or_path).is_file() {
        return Ok(std::fs::read(url_or_path)?);
    }
    let opts = OpenOptions { timeout: Some(timeout), ..Default::default() };
    let resp = browser.open(url_or_path, &opts)?;
    Ok(resp.into_bytes())
}

/// Port of the `ext = mu.rpartition('.')[-1]; if '?' in ext: ext = ''`
/// / `ext.lower() if ext else 'jpg'` logic shared by `_download_cover`
/// and `_download_masthead`.
fn guess_ext(url_or_path: &str) -> String {
    match url_or_path.rsplit('.').next() {
        Some(ext) if !ext.is_empty() && !ext.contains('?') => ext.to_lowercase(),
        _ => "jpg".to_string(),
    }
}

// ===================================================================
// Cover
// ===================================================================

/// Port of `_download_cover`/`download_cover`: fetches, optionally
/// border-composites, and saves the recipe's cover image to
/// `output_dir/cover.jpg`. Returns `None` on any failure or missing
/// cover -- matching real Python's own catch-all-and-log-only error
/// handling (`self.cover_path = None`).
pub fn download_cover(browser: &Browser, hooks: &dyn NewsRecipeHooks, output_dir: &Path, timeout: Duration) -> Option<PathBuf> {
    let cover_url = hooks.get_cover_url()?;
    let cdata = fetch_bytes(browser, timeout, &cover_url).ok()?;
    if cdata.is_empty() {
        return None;
    }
    let ext = guess_ext(&cover_url);
    if ext == "pdf" {
        // Disclosed gap: no PDF cover-metadata extraction path exists
        // yet anywhere in this crate -- see this module's own doc.
        return None;
    }

    let margins = &hooks.config().cover_margins;
    let cdata = if margins.horizontal != 0 || margins.vertical != 0 {
        let h = margins.horizontal.max(0) as u32;
        let v = margins.vertical.max(0) as u32;
        let bordered = add_borders_to_image(&cdata, h, v, h, v, &margins.color).ok()?;
        image_to_data(&bordered, 95)
    } else {
        cdata
    };

    let cpath = output_dir.join("cover.jpg");
    save_cover_data_to(&cdata, &cpath).ok()?;
    Some(cpath)
}

/// Port of `default_cover`.
pub fn default_cover(title: &str, date_str: &str, db: &Arc<fontdb::Database>) -> Option<Vec<u8>> {
    create_cover(title, &[date_str.to_string()], None, 1.0, &CoverPrefs::default(), db, false)
}

// ===================================================================
// Masthead
// ===================================================================

/// Port of `_download_masthead`/`download_masthead`: fetches a
/// masthead image to a temporary `masthead_source.<ext>` file, then
/// fits it onto `output_dir/mastheadImage.jpg` via the already-real
/// [`prepare_masthead_image`]. Returns `None` on any failure --
/// matching real Python's own catch-all-and-log-only handling.
pub fn download_masthead(browser: &Browser, timeout: Duration, masthead_url: &str, output_dir: &Path) -> Option<PathBuf> {
    let mdata = fetch_bytes(browser, timeout, masthead_url).ok()?;
    let mpath = output_dir.join(format!("masthead_source.{}", guess_ext(masthead_url)));
    std::fs::write(&mpath, &mdata).ok()?;

    let outfile = output_dir.join("mastheadImage.jpg");
    let result = prepare_masthead_image(&mpath, &outfile, MI_WIDTH, MI_HEIGHT);
    let _ = std::fs::remove_file(&mpath);
    result.ok()?;
    Some(outfile)
}

/// Port of `default_masthead_image`.
pub fn default_masthead_image(db: &Arc<fontdb::Database>, masthead_title: &str) -> Option<Vec<u8>> {
    generate_masthead(db, masthead_title, MI_WIDTH, MI_HEIGHT, None)
}

/// Port of `resolve_masthead`: tries the recipe-supplied masthead URL
/// first, falling back to a synthesized masthead image if that's
/// absent or fails.
pub fn resolve_masthead(browser: &Browser, hooks: &dyn NewsRecipeHooks, output_dir: &Path, timeout: Duration, db: &Arc<fontdb::Database>) -> Option<PathBuf> {
    if let Some(murl) = hooks.get_masthead_url() {
        if let Some(path) = download_masthead(browser, timeout, &murl, output_dir) {
            return Some(path);
        }
    }
    let title = hooks.get_masthead_title();
    let data = default_masthead_image(db, &title)?;
    let outfile = output_dir.join("mastheadImage.jpg");
    std::fs::write(&outfile, &data).ok()?;
    Some(outfile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::feeds::recipe::RecipeConfig;
    use std::io::Write;
    use std::sync::OnceLock;

    struct TestRecipe(RecipeConfig);
    impl NewsRecipeHooks for TestRecipe {
        fn config(&self) -> &RecipeConfig {
            &self.0
        }
    }

    fn test_db() -> &'static Arc<fontdb::Database> {
        static DB: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
        DB.get_or_init(|| {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
    }

    fn make_png(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(width, height, image::Rgb(color));
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png).unwrap();
        buf
    }

    #[test]
    fn add_borders_to_image_leaves_a_zero_margin_image_unchanged() {
        let data = make_png(10, 6, [1, 2, 3]);
        let out = add_borders_to_image(&data, 0, 0, 0, 0, "#ffffff").unwrap();
        assert_eq!(out.dimensions(), (10, 6));
        assert_eq!(out.get_pixel(0, 0).0[..3], [1, 2, 3]);
    }

    #[test]
    fn add_borders_to_image_pads_with_the_requested_color() {
        let data = make_png(4, 4, [10, 20, 30]);
        let out = add_borders_to_image(&data, 2, 3, 5, 1, "#00ff00").unwrap();
        assert_eq!(out.dimensions(), (4 + 2 + 5, 4 + 3 + 1));
        // Inside the original region.
        assert_eq!(out.get_pixel(2, 3).0[..3], [10, 20, 30]);
        // Inside the top-left border region.
        assert_eq!(out.get_pixel(0, 0).0[..3], [0, 255, 0]);
        // Inside the bottom-right border region.
        let (w, h) = out.dimensions();
        assert_eq!(out.get_pixel(w - 1, h - 1).0[..3], [0, 255, 0]);
    }

    #[test]
    fn add_borders_to_image_falls_back_to_white_for_an_unparseable_color() {
        let data = make_png(2, 2, [0, 0, 0]);
        let out = add_borders_to_image(&data, 1, 1, 1, 1, "not-a-color").unwrap();
        assert_eq!(out.get_pixel(0, 0).0[..3], [255, 255, 255]);
    }

    #[test]
    fn image_to_data_round_trips_through_jpeg() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 8, image::Rgb([200, 100, 50])));
        let data = image_to_data(&img, 90);
        let decoded = image::load_from_memory(&data).unwrap();
        assert_eq!(decoded.dimensions(), (8, 8));
    }

    #[test]
    fn save_cover_data_to_writes_a_jpeg_file() {
        let dir = std::env::temp_dir().join(format!("calibre-oxide-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = make_png(5, 5, [9, 9, 9]);
        let out = dir.join("cover.jpg");
        save_cover_data_to(&png, &out).unwrap();
        let decoded = image::open(&out).unwrap();
        assert_eq!(decoded.dimensions(), (5, 5));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn guess_ext_strips_a_query_string() {
        assert_eq!(guess_ext("http://example.com/cover.jpg?x=1"), "jpg");
        assert_eq!(guess_ext("http://example.com/cover.PNG"), "png");
        // No `.` anywhere after the host's own dot falls back to it --
        // matches real Python's own naive whole-string `rpartition('.')`
        // (not path-basename-aware), same quirk upstream has.
        assert_eq!(guess_ext("http://example.com/cover"), "com/cover");
        // No `.` at all: Python's `rpartition('.')` returns the whole
        // string as the last element (not an empty string), so this
        // falls through to it verbatim too, matching real behavior.
        assert_eq!(guess_ext("cover"), "cover");
        assert_eq!(guess_ext(""), "jpg");
    }

    #[test]
    fn download_cover_reads_a_local_file_and_saves_it() {
        let dir = std::env::temp_dir().join(format!("calibre-oxide-test-cover-local-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("source.png");
        std::fs::write(&src, make_png(6, 6, [1, 1, 1])).unwrap();

        // `get_cover_url` reads from `RecipeConfig` via the trait's
        // hook default (`None`), so exercise the real fetch path
        // through a small wrapper hook instead.
        struct CoverRecipe(RecipeConfig, String);
        impl NewsRecipeHooks for CoverRecipe {
            fn config(&self) -> &RecipeConfig {
                &self.0
            }
            fn get_cover_url(&self) -> Option<String> {
                Some(self.1.clone())
            }
        }
        let hooks = CoverRecipe(RecipeConfig::default(), src.to_string_lossy().to_string());
        let browser = Browser::new("", &[], true);

        let result = download_cover(&browser, &hooks, &dir, Duration::from_secs(5));
        assert!(result.is_some());
        let cover_path = result.unwrap();
        assert_eq!(cover_path, dir.join("cover.jpg"));
        let decoded = image::open(&cover_path).unwrap();
        assert_eq!(decoded.dimensions(), (6, 6));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn download_cover_applies_border_margins() {
        let dir = std::env::temp_dir().join(format!("calibre-oxide-test-cover-margins-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("source.png");
        std::fs::write(&src, make_png(6, 6, [1, 1, 1])).unwrap();

        struct CoverRecipe(RecipeConfig, String);
        impl NewsRecipeHooks for CoverRecipe {
            fn config(&self) -> &RecipeConfig {
                &self.0
            }
            fn get_cover_url(&self) -> Option<String> {
                Some(self.1.clone())
            }
        }
        let mut cfg = RecipeConfig::default();
        cfg.cover_margins.horizontal = 2;
        cfg.cover_margins.vertical = 3;
        let hooks = CoverRecipe(cfg, src.to_string_lossy().to_string());
        let browser = Browser::new("", &[], true);

        let cover_path = download_cover(&browser, &hooks, &dir, Duration::from_secs(5)).unwrap();
        let decoded = image::open(&cover_path).unwrap();
        assert_eq!(decoded.dimensions(), (6 + 2 + 2, 6 + 3 + 3));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn download_cover_returns_none_for_a_pdf_extension() {
        let dir = std::env::temp_dir().join(format!("calibre-oxide-test-cover-pdf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("source.pdf");
        let mut f = std::fs::File::create(&src).unwrap();
        f.write_all(b"%PDF-1.4 not a real pdf").unwrap();

        struct CoverRecipe(RecipeConfig, String);
        impl NewsRecipeHooks for CoverRecipe {
            fn config(&self) -> &RecipeConfig {
                &self.0
            }
            fn get_cover_url(&self) -> Option<String> {
                Some(self.1.clone())
            }
        }
        let hooks = CoverRecipe(RecipeConfig::default(), src.to_string_lossy().to_string());
        let browser = Browser::new("", &[], true);

        assert!(download_cover(&browser, &hooks, &dir, Duration::from_secs(5)).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn download_cover_returns_none_with_no_cover_url() {
        let hooks = TestRecipe(RecipeConfig::default());
        let browser = Browser::new("", &[], true);
        let dir = std::env::temp_dir();
        assert!(download_cover(&browser, &hooks, &dir, Duration::from_secs(5)).is_none());
    }

    #[test]
    fn default_cover_generates_real_png_bytes() {
        let data = default_cover("Test Weekly", "2026-09-10", test_db());
        let bytes = data.expect("default_cover should produce bytes with the system font database");
        assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn default_masthead_image_generates_real_jpeg_or_png_bytes() {
        let data = default_masthead_image(test_db(), "Test Weekly");
        assert!(data.is_some());
    }

    #[test]
    fn resolve_masthead_falls_back_to_synthesis_when_no_masthead_url_is_set() {
        let dir = std::env::temp_dir().join(format!("calibre-oxide-test-masthead-fallback-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let hooks = TestRecipe(RecipeConfig { title: "Test Weekly".to_string(), ..Default::default() });
        let browser = Browser::new("", &[], true);

        let result = resolve_masthead(&browser, &hooks, &dir, Duration::from_secs(5), test_db());
        assert_eq!(result, Some(dir.join("mastheadImage.jpg")));
        assert!(dir.join("mastheadImage.jpg").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_masthead_uses_a_local_masthead_url_when_set() {
        let dir = std::env::temp_dir().join(format!("calibre-oxide-test-masthead-local-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("masthead.png");
        std::fs::write(&src, make_png(600, 60, [5, 5, 5])).unwrap();

        let hooks = TestRecipe(RecipeConfig {
            title: "Test Weekly".to_string(),
            masthead_url: Some(src.to_string_lossy().to_string()),
            ..Default::default()
        });
        let browser = Browser::new("", &[], true);

        let result = resolve_masthead(&browser, &hooks, &dir, Duration::from_secs(5), test_db());
        let masthead_path = result.expect("a local masthead_url should resolve");
        assert_eq!(masthead_path, dir.join("mastheadImage.jpg"));
        // The temporary source-extension copy must be cleaned up.
        assert!(!dir.join("masthead_source.png").exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
