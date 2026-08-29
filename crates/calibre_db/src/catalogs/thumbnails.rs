//! Port of `epub_mobi_builder.py`'s thumbnail-generation slice
//! (`generate_thumbnail`, `generate_thumbnails`, `confirm_thumbs_archive`)
//! -- part of cluster E of the `epub_mobi_builder.py` port (issue #57;
//! see `epub_mobi_builder.rs`'s own module doc for clusters A-D, already
//! complete, and `opf.rs`/`ncx.rs`/`output_profiles.rs` for the rest of
//! cluster E). Split into its own file for the same size reason as
//! `ncx.rs`/`opf.rs`.
//!
//! Unlike every other file in this port, these functions are genuinely
//! about file I/O and image processing rather than pure content
//! transforms -- matching `output.rs`'s own precedent as "the one
//! exception" to this module's usual "pure function, caller does I/O"
//! convention.
//!
//! # Not ported: `generate_masthead_image`
//!
//! Renders `self.opts.catalog_title` as text onto a blank image using a
//! TrueType font (`PIL.ImageFont`/`ImageDraw`), for the Kindle-periodical
//! masthead. This crate has no font-rasterization/text-layout dependency
//! anywhere (the `image` crate this file newly depends on handles pixel
//! data, not glyph rendering) -- adding one is a real, separately-scoped
//! decision, not a small gap-fill like `calibre_utils::html2text` or
//! `Cache::all_tags` were. Given this only matters for Kindle/MOBI
//! periodical-format catalogs specifically (a narrow device-format
//! feature, not core catalog generation), it's left unported rather than
//! stubbed with a blank or wrong-looking image.
//!
//! # Disclosed simplifications
//!
//! - **`scale_image`'s CRC-32 cache key is replaced with a BLAKE3 hash**
//!   (`blake3` is already a dependency of this crate; `crc32fast`/`crc`
//!   is not). The exact hash algorithm is an internal cache-key
//!   implementation detail with no external compatibility requirement --
//!   this port's thumbnail cache is never shared with or read by a real
//!   Python-generated one, so nothing depends on it being literally
//!   CRC-32.
//! - **`generate_thumbnail`'s "no uuid, no-op" bug is preserved, not
//!   fixed.** Tracing upstream's indentation precisely: the entire
//!   cache-check/generate/write/cache-save body lives inside `if uuid:`
//!   -- a book with no `uuid` causes `generate_thumbnail` to do
//!   *nothing* (no exception, no file written), which its caller
//!   (`generate_thumbnails`) then silently treats as success (appending
//!   a thumbnail filename to `self.thumbs` that was never actually
//!   written). Every real calibre book has a uuid in practice (assigned
//!   automatically on import), so this is the same "narrow, essentially
//!   unreachable with real data" bar `bibtex.rs`'s ISBN-13 hyphenation
//!   quirks were preserved under -- not the "ordinary case, visible
//!   defect" bar that's warranted a fix elsewhere in this port (compare
//!   `process_exclusions`'s duplicate-survivor fix).
//! - **No default-cover fallback resource is bundled.** Upstream's
//!   `generate_thumbnails` falls back to `I('default_cover.png')` (a
//!   calibre-bundled resource) when a book's cover is missing or
//!   invalid; this workspace has no established convention yet for
//!   bundling binary resources into a crate (checked: no crate embeds
//!   any font/image resource anywhere). [`generate_thumbnails`] instead
//!   takes an optional `default_cover_path` the caller supplies; without
//!   one, a book with no usable cover is simply skipped from the
//!   returned thumbnail list rather than getting a substitute image.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use calibre_ebooks::oeb::transforms::rescale::fit_image;
use image::DynamicImage;
use serde_json::Value;

/// Port of `calibre.utils.img.scale_image`, narrowed to the JPEG,
/// aspect-preserving case (`scale_image`'s only real call shape in
/// `epub_mobi_builder.py`: `scale_image(data, width=.., height=..)`,
/// upstream's own defaults for `preserve_aspect_ratio`/`as_png`).
/// Transparency is alpha-blended with white before JPEG encoding
/// (JPEG has no alpha channel), matching upstream's documented behavior.
pub fn scale_image(data: &[u8], width: u32, height: u32, compression_quality: u8) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory(data)?;
    let (scaled, nw, nh) = fit_image(img.width() as f64, img.height() as f64, width as f64, height as f64);
    let resized =
        if scaled { img.resize_exact(nw.max(1) as u32, nh.max(1) as u32, image::imageops::FilterType::Lanczos3) } else { img };
    let rgb = blend_with_white(&resized);
    let (w, h) = (rgb.width(), rgb.height());

    let mut buf = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, compression_quality);
    encoder.encode_image(&DynamicImage::ImageRgb8(rgb))?;
    Ok((w, h, buf))
}

fn blend_with_white(img: &DynamicImage) -> image::RgbImage {
    match img {
        DynamicImage::ImageRgba8(rgba) => {
            let mut out = image::RgbImage::new(rgba.width(), rgba.height());
            for (x, y, px) in rgba.enumerate_pixels() {
                let [r, g, b, a] = px.0;
                let af = a as f32 / 255.0;
                let blend = |c: u8| ((c as f32) * af + 255.0 * (1.0 - af)).round() as u8;
                out.put_pixel(x, y, image::Rgb([blend(r), blend(g), blend(b)]));
            }
            out
        }
        other => other.to_rgb8(),
    }
}

const ARCHIVE_MARKER: &str = "Catalog Thumbs Archive";

fn write_fresh_archive(thumbs_path: &Path) -> anyhow::Result<()> {
    let file = fs::File::create(thumbs_path)?;
    let mut zw = zip::ZipWriter::new(file);
    zw.start_file(ARCHIVE_MARKER, zip::write::FileOptions::default())?;
    zw.finish()?;
    Ok(())
}

/// Port of `confirm_thumbs_archive`.
pub fn confirm_thumbs_archive(cache_dir: &Path, thumbs_path: &Path, thumb_width: f64) -> anyhow::Result<()> {
    fs::create_dir_all(cache_dir)?;

    if !thumbs_path.exists() {
        return write_fresh_archive(thumbs_path);
    }

    let cached_thumb_width: f64 = match fs::File::open(thumbs_path).ok().and_then(|f| zip::ZipArchive::new(f).ok()) {
        Some(mut archive) => match archive.by_name("thumb_width") {
            Ok(mut entry) => {
                let mut s = String::new();
                entry.read_to_string(&mut s).ok();
                s.parse().unwrap_or(-1.0)
            }
            Err(_) => -1.0,
        },
        None => {
            let _ = fs::remove_file(thumbs_path);
            -1.0
        }
    };

    if cached_thumb_width != thumb_width {
        write_fresh_archive(thumbs_path)?;
    }
    Ok(())
}

/// Port of `generate_thumbnail`. See this module's doc for why a
/// missing/empty `uuid` makes this a documented no-op rather than an
/// error.
pub fn generate_thumbnail(
    cover_path: &Path,
    uuid: Option<&str>,
    thumbs_path: &Path,
    image_dir: &Path,
    thumb_file: &str,
    thumb_width: u32,
    thumb_height: u32,
) -> anyhow::Result<()> {
    let Some(uuid) = uuid.filter(|u| !u.is_empty()) else {
        return Ok(());
    };

    let data = fs::read(cover_path)?;
    let cover_hash = blake3::hash(&data).to_hex().to_string();
    let key = format!("{uuid}{cover_hash}");
    fs::create_dir_all(image_dir)?;
    let out_path = image_dir.join(thumb_file);

    if let Some(mut archive) = fs::File::open(thumbs_path).ok().and_then(|f| zip::ZipArchive::new(f).ok()) {
        if let Ok(mut entry) = archive.by_name(&key) {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            fs::write(&out_path, &buf)?;
            return Ok(());
        }
    }

    let (_, _, thumb_data) = scale_image(&data, thumb_width, thumb_height, 70)?;
    fs::write(&out_path, &thumb_data)?;

    if let Ok(file) = fs::OpenOptions::new().read(true).write(true).open(thumbs_path) {
        if let Ok(mut zw) = zip::ZipWriter::new_append(file) {
            if zw.start_file(&key, zip::write::FileOptions::default()).is_ok() {
                let _ = zw.write_all(&thumb_data);
                let _ = zw.finish();
            }
        }
    }
    Ok(())
}

fn book_id_i64(book: &Value) -> i64 {
    book.get("id").and_then(|v| v.as_i64()).unwrap_or_default()
}

/// Port of `generate_thumbnails`. See this module's doc for the
/// `default_cover_path` simplification. Returns the thumbnail filename
/// list (`self.thumbs`) -- always starts with `"thumbnail_default.jpg"`
/// only when `default_cover_path` is provided and a default thumbnail
/// was actually generated from it, matching upstream's own list shape
/// otherwise closely (upstream unconditionally seeds the list with that
/// name even before confirming the default thumbnail exists; this port
/// only includes it once real work backs it, since there's no bundled
/// resource to unconditionally promise one exists).
pub fn generate_thumbnails(
    books_by_title: &[Value],
    catalog_path: &Path,
    thumbs_path: &Path,
    thumb_width: u32,
    thumb_height: u32,
    default_cover_path: Option<&Path>,
) -> anyhow::Result<Vec<String>> {
    let image_dir = catalog_path.join("images");
    let mut thumbs: Vec<String> = Vec::new();
    let mut default_thumb_generated = false;

    for title in books_by_title {
        let book_id = book_id_i64(title);
        let thumb_file = format!("thumbnail_{book_id}.jpg");
        let uuid = title.get("uuid").and_then(|v| v.as_str());
        let cover = title.get("cover").and_then(|v| v.as_str()).map(PathBuf::from);

        let generated = match &cover {
            Some(cover_path) if cover_path.exists() => {
                generate_thumbnail(cover_path, uuid, thumbs_path, &image_dir, &thumb_file, thumb_width, thumb_height).is_ok()
            }
            _ => false,
        };

        if generated {
            thumbs.push(thumb_file);
        } else if let Some(default_cover) = default_cover_path {
            if generate_thumbnail(default_cover, uuid, thumbs_path, &image_dir, "thumbnail_default.jpg", thumb_width, thumb_height)
                .is_ok()
            {
                default_thumb_generated = true;
            }
        }
    }

    if default_thumb_generated {
        thumbs.insert(0, "thumbnail_default.jpg".to_string());
    }

    if let Ok(file) = fs::OpenOptions::new().read(true).write(true).open(thumbs_path) {
        if let Ok(mut zw) = zip::ZipWriter::new_append(file) {
            if zw.start_file("thumb_width", zip::write::FileOptions::default()).is_ok() {
                let _ = zw.write_all(thumb_width.to_string().as_bytes());
                let _ = zw.finish();
            }
        }
    }

    Ok(thumbs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn a_small_png() -> Vec<u8> {
        let img = image::RgbImage::from_fn(20, 30, |x, y| image::Rgb([(x * 10) as u8, (y * 5) as u8, 128]));
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png).unwrap();
        buf
    }

    #[test]
    fn scale_image_produces_valid_jpeg_within_the_bounding_box() {
        let png = a_small_png();
        let (w, h, jpeg) = scale_image(&png, 10, 10, 70).unwrap();
        assert!(w <= 10 && h <= 10, "{w}x{h}");
        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert_eq!(decoded.width(), w);
        assert_eq!(decoded.height(), h);
    }

    #[test]
    fn scale_image_preserves_aspect_ratio() {
        // 20x30 fit into a 10x10 box should end up 6x10 or 7x10-ish,
        // never stretched to exactly 10x10.
        let png = a_small_png();
        let (w, h, _) = scale_image(&png, 10, 10, 70).unwrap();
        assert!(w < 10 || h < 10, "{w}x{h} should not fill the box on both axes");
    }

    #[test]
    fn confirm_thumbs_archive_creates_a_fresh_archive_when_missing() {
        let dir = tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let thumbs_path = cache_dir.join("thumbs.zip");
        confirm_thumbs_archive(&cache_dir, &thumbs_path, 1.0).unwrap();
        assert!(thumbs_path.exists());
    }

    #[test]
    fn confirm_thumbs_archive_invalidates_on_width_mismatch() {
        let dir = tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let thumbs_path = cache_dir.join("thumbs.zip");
        confirm_thumbs_archive(&cache_dir, &thumbs_path, 1.0).unwrap();

        // Simulate generate_thumbnails having written thumb_width=1.0.
        {
            let file = fs::OpenOptions::new().read(true).write(true).open(&thumbs_path).unwrap();
            let mut zw = zip::ZipWriter::new_append(file).unwrap();
            zw.start_file("thumb_width", zip::write::FileOptions::default()).unwrap();
            zw.write_all(b"1").unwrap();
            zw.finish().unwrap();
        }

        confirm_thumbs_archive(&cache_dir, &thumbs_path, 2.0).unwrap();

        // Archive was recreated -- thumb_width entry no longer present.
        let file = fs::File::open(&thumbs_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        assert!(archive.by_name("thumb_width").is_err());
    }

    #[test]
    fn generate_thumbnail_is_a_no_op_without_a_uuid() {
        let dir = tempdir().unwrap();
        let cover_path = dir.path().join("cover.png");
        fs::write(&cover_path, a_small_png()).unwrap();
        let image_dir = dir.path().join("images");
        let thumbs_path = dir.path().join("thumbs.zip");

        generate_thumbnail(&cover_path, None, &thumbs_path, &image_dir, "thumbnail_1.jpg", 60, 80).unwrap();
        assert!(!image_dir.join("thumbnail_1.jpg").exists());
    }

    #[test]
    fn generate_thumbnail_writes_a_jpeg_file_when_uuid_is_present() {
        let dir = tempdir().unwrap();
        let cover_path = dir.path().join("cover.png");
        fs::write(&cover_path, a_small_png()).unwrap();
        let image_dir = dir.path().join("images");
        let thumbs_path = dir.path().join("thumbs.zip");
        confirm_thumbs_archive(dir.path(), &thumbs_path, 1.0).unwrap();

        generate_thumbnail(&cover_path, Some("abc-uuid"), &thumbs_path, &image_dir, "thumbnail_1.jpg", 60, 80).unwrap();
        assert!(image_dir.join("thumbnail_1.jpg").exists());
    }

    #[test]
    fn generate_thumbnail_reuses_the_cached_entry_on_a_second_call() {
        let dir = tempdir().unwrap();
        let cover_path = dir.path().join("cover.png");
        fs::write(&cover_path, a_small_png()).unwrap();
        let image_dir = dir.path().join("images");
        let thumbs_path = dir.path().join("thumbs.zip");
        confirm_thumbs_archive(dir.path(), &thumbs_path, 1.0).unwrap();

        generate_thumbnail(&cover_path, Some("abc-uuid"), &thumbs_path, &image_dir, "thumbnail_1.jpg", 60, 80).unwrap();
        let first_write = fs::read(image_dir.join("thumbnail_1.jpg")).unwrap();
        fs::remove_file(image_dir.join("thumbnail_1.jpg")).unwrap();

        generate_thumbnail(&cover_path, Some("abc-uuid"), &thumbs_path, &image_dir, "thumbnail_1.jpg", 60, 80).unwrap();
        let second_write = fs::read(image_dir.join("thumbnail_1.jpg")).unwrap();
        assert_eq!(first_write, second_write);
    }

    #[test]
    fn generate_thumbnails_skips_books_with_no_cover_and_no_default() {
        let dir = tempdir().unwrap();
        let thumbs_path = dir.path().join("thumbs.zip");
        confirm_thumbs_archive(dir.path(), &thumbs_path, 1.0).unwrap();

        let books = vec![serde_json::json!({"id": 1, "uuid": "u1", "cover": Value::Null})];
        let thumbs = generate_thumbnails(&books, dir.path(), &thumbs_path, 60, 80, None).unwrap();
        assert!(thumbs.is_empty());
    }

    #[test]
    fn generate_thumbnails_includes_a_real_cover() {
        let dir = tempdir().unwrap();
        let thumbs_path = dir.path().join("thumbs.zip");
        confirm_thumbs_archive(dir.path(), &thumbs_path, 1.0).unwrap();

        let cover_path = dir.path().join("cover.png");
        fs::write(&cover_path, a_small_png()).unwrap();

        let books = vec![serde_json::json!({"id": 1, "uuid": "u1", "cover": cover_path.to_str().unwrap()})];
        let thumbs = generate_thumbnails(&books, dir.path(), &thumbs_path, 60, 80, None).unwrap();
        assert_eq!(thumbs, vec!["thumbnail_1.jpg".to_string()]);
        assert!(dir.path().join("images/thumbnail_1.jpg").exists());
    }
}
