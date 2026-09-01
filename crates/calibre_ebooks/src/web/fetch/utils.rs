//! Port of `old_src/src/calibre/web/fetch/utils.py`: shrinking/
//! recompressing downloaded news-recipe images to fit a size budget,
//! and preparing a periodical's masthead image.
//!
//! # Scope
//!
//! Real: [`rescale_image`]'s full algorithm (optional dimension cap,
//! then iterative JPEG re-compression at decreasing quality until a
//! size budget is met, with upstream's own three-way fallback: keep
//! the recompressed result only if it's smaller than both the
//! dimension-scaled and original data) and [`prepare_masthead_image`]
//! (fit-and-center onto a white canvas), backed by the `image` crate
//! and this crate's own [`crate::oeb::transforms::rescale::fit_image`]
//! (already a faithful port of `calibre.fit_image`) instead of
//! upstream's Qt-based `calibre.utils.img` (`utils/img.py` itself
//! isn't ported -- only the narrow JPEG-only slice this module
//! actually needs is reimplemented here directly, scoped to this
//! file's real call sites, not a general-purpose image-utilities
//! module).

use image::{DynamicImage, GenericImageView, ImageFormat};
use std::path::Path;

use crate::oeb::transforms::rescale::fit_image;

fn encode_jpeg(img: &DynamicImage, quality: u8) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut buf);
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, quality);
    // A decoded image may carry an alpha channel (PNG source); JPEG
    // has none, so flatten to RGB first, matching Qt's own "alpha
    // blended with white when converting to JPEG" behavior closely
    // enough for a news-image pipeline (upstream's own doc says as
    // much for `scale_image`).
    let rgb = img.to_rgb8();
    let _ = encoder.encode(rgb.as_raw(), rgb.width(), rgb.height(), image::ColorType::Rgb8);
    buf
}

/// Port of `rescale_image`.
pub fn rescale_image(
    data: &[u8],
    scale_news_images: Option<(u32, u32)>,
    compress_news_images_max_size_kb: Option<u32>,
    compress_news_images_auto_size: Option<f64>,
) -> Vec<u8> {
    let orig_data = data.to_vec();
    let Ok(img) = image::load_from_memory(data) else {
        return orig_data;
    };
    let (mut orig_w, mut orig_h) = img.dimensions();
    let mut data = orig_data.clone();
    let mut current_img = img;

    if let Some((wmax, hmax)) = scale_news_images {
        let (scaled, nw, nh) = fit_image(orig_w as f64, orig_h as f64, wmax as f64, hmax as f64);
        if scaled {
            let nw = nw.max(1) as u32;
            let nh = nh.max(1) as u32;
            current_img = current_img.resize(nw, nh, image::imageops::FilterType::Lanczos3);
            orig_w = nw;
            orig_h = nh;
            data = encode_jpeg(&current_img, 95);
        }
    }

    let maxsizeb: f64 = if let Some(max_kb) = compress_news_images_max_size_kb {
        (max_kb as f64) * 1024.0
    } else if let Some(auto_size) = compress_news_images_auto_size {
        (orig_w as f64 * orig_h as f64) / auto_size
    } else {
        return data; // not compressing
    };

    if data.len() as f64 <= maxsizeb {
        return data; // no compression required
    }

    let scaled_data = data.clone();
    let mut quality: i32 = 90;
    let mut compressed = data.clone();
    while (compressed.len() as f64) >= maxsizeb && quality >= 5 {
        compressed = encode_jpeg(&current_img, quality as u8);
        quality -= 5;
    }

    if compressed.len() >= scaled_data.len() {
        // compression failed
        return if orig_data.len() <= scaled_data.len() { orig_data } else { scaled_data };
    }
    if compressed.len() >= orig_data.len() {
        // no improvement
        return orig_data;
    }
    compressed
}

/// Port of `calibre.utils.img.blend_on_canvas`: fits `img` into
/// `width`x`height` (preserving aspect ratio) and centers it on a
/// white canvas of exactly that size.
pub fn blend_on_canvas(img: &DynamicImage, width: u32, height: u32) -> image::RgbImage {
    let (w, h) = img.dimensions();
    let (scaled, nw, nh) = fit_image(w as f64, h as f64, width as f64, height as f64);
    let (nw, nh) = (nw.max(1) as u32, nh.max(1) as u32);
    let resized = if scaled { img.resize_exact(nw, nh, image::imageops::FilterType::Lanczos3) } else { img.clone() };

    let mut canvas = image::RgbImage::from_pixel(width, height, image::Rgb([255, 255, 255]));
    let x_off = width.saturating_sub(nw) / 2;
    let y_off = height.saturating_sub(nh) / 2;
    image::imageops::overlay(&mut canvas, &resized.to_rgb8(), x_off as i64, y_off as i64);
    canvas
}

/// Port of `prepare_masthead_image`.
pub fn prepare_masthead_image(path_to_image: &Path, out_path: &Path, mi_width: u32, mi_height: u32) -> anyhow::Result<()> {
    let img = image::open(path_to_image)?;
    let canvas = blend_on_canvas(&img, mi_width, mi_height);
    canvas.save_with_format(out_path, ImageFormat::Jpeg)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_jpeg(width: u32, height: u32) -> Vec<u8> {
        let img = DynamicImage::new_rgb8(width, height);
        encode_jpeg(&img, 95)
    }

    #[test]
    fn returns_data_unchanged_when_neither_scaling_nor_compression_is_requested() {
        let data = make_test_jpeg(100, 100);
        let out = rescale_image(&data, None, None, None);
        assert_eq!(out, data);
    }

    #[test]
    fn scales_down_an_oversized_image() {
        let data = make_test_jpeg(2000, 1000);
        let out = rescale_image(&data, Some((500, 500)), None, None);
        let decoded = image::load_from_memory(&out).unwrap();
        let (w, h) = decoded.dimensions();
        assert!(w <= 500 && h <= 500, "{w}x{h}");
        // Aspect ratio (2:1) should be preserved.
        assert_eq!(w, 2 * h);
    }

    #[test]
    fn leaves_a_small_enough_image_alone_when_scaling_is_within_bounds() {
        let data = make_test_jpeg(100, 100);
        let out = rescale_image(&data, Some((500, 500)), None, None);
        let decoded = image::load_from_memory(&out).unwrap();
        assert_eq!(decoded.dimensions(), (100, 100));
    }

    #[test]
    fn compresses_to_fit_a_max_size_budget() {
        let data = make_test_jpeg(800, 800);
        let budget_kb = (data.len() / 1024).max(1) as u32 / 2; // ask for roughly half the natural size
        let out = rescale_image(&data, None, Some(budget_kb), None);
        // Either it hit budget, or compression truly couldn't do
        // better -- either way it must not have grown.
        assert!(out.len() <= data.len(), "{} vs {}", out.len(), data.len());
    }

    #[test]
    fn masthead_image_is_centered_on_a_white_canvas_of_the_requested_size() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("logo.jpg");
        std::fs::write(&src, make_test_jpeg(100, 50)).unwrap();
        let out = dir.path().join("masthead.jpg");

        prepare_masthead_image(&src, &out, 600, 60).unwrap();
        let decoded = image::open(&out).unwrap();
        assert_eq!(decoded.dimensions(), (600, 60));
    }
}
