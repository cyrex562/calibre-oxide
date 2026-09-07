//! Port of `calibre.utils.imageops` (issue #67, split into #569-571):
//! real Qt-`QImage`-based image operations, ported against the
//! `image` crate's `RgbaImage`.
//!
//! This file (issue #569) covers the real pixel/histogram-level
//! operations from `imageops.cpp`: [`remove_borders`], [`grayscale`],
//! [`overlay`], [`has_transparent_pixels`], [`set_opacity`],
//! [`texture_image`], [`dominant_color`], [`normalize`]. Convolution
//! filters (`gaussian_sharpen`/`gaussian_blur`/`despeckle`/`oil_paint`,
//! #570) and color quantization/eink dithering (`quantize.cpp`/
//! `ordered_dither.cpp`, #571) are separate, dependency-ordered
//! follow-up scope.
//!
//! # Disclosed narrowings
//!
//! - [`overlay`]/[`texture_image`]'s real upstream signatures require
//!   the canvas to have NO alpha channel (`QImage::Format_RGB32`, a
//!   real distinct pixel format from `Format_ARGB32` in Qt) --
//!   `overlay` even explicitly throws if the canvas has one. This
//!   port's `RgbaImage` always structurally carries an alpha channel,
//!   so that precondition isn't (and can't cleanly be) validated here;
//!   the real blend MATH (which already assumes an opaque canvas) is
//!   performed exactly as upstream, matching the documented contract
//!   rather than enforcing it.
//! - [`normalize`]'s real per-channel lookup-table build has a genuine
//!   upstream quirk: when a channel's `low == high`, the single table
//!   entry at `i == low == high` is left at its default (`0`) instead
//!   of being explicitly computed. Reproduced as-is rather than
//!   "fixed" -- see that function's own comment for why it's
//!   observably inert (the write-back phase bypasses the table
//!   entirely for any channel where `low == high`).

use std::collections::{HashMap, HashSet};

use image::{Rgba, RgbaImage};

/// Port of Qt's `qGray(r, g, b)`.
fn q_gray(r: u8, g: u8, b: u8) -> u8 {
    ((r as u32 * 11 + g as u32 * 16 + b as u32 * 5) / 32) as u8
}

/// Port of `grayscale`.
pub fn grayscale(image: &RgbaImage) -> RgbaImage {
    let mut out = image.clone();
    for p in out.pixels_mut() {
        let gray = q_gray(p[0], p[1], p[2]);
        p[0] = gray;
        p[1] = gray;
        p[2] = gray;
    }
    out
}

/// Port of `read_border_row`: the number of consecutive rows (from
/// the top or bottom edge) that are internally homogeneous (every
/// pixel within `fuzz` of the row's own average color) AND close to
/// the very first such row's average color.
fn read_border_row(img: &RgbaImage, fuzz: f64, top: bool) -> u32 {
    let (width, height) = img.dimensions();
    let mut ans = 0u32;
    let mut first_avg = (0.0f64, 0.0f64, 0.0f64);
    let rows: Box<dyn Iterator<Item = u32>> = if top { Box::new(0..height) } else { Box::new((0..height).rev()) };

    for (idx, r) in rows.enumerate() {
        let mut reds = vec![0.0f64; width as usize];
        let mut greens = vec![0.0f64; width as usize];
        let mut blues = vec![0.0f64; width as usize];
        let (mut rs, mut gs, mut bs) = (0.0f64, 0.0f64, 0.0f64);
        for c in 0..width as usize {
            let p = img.get_pixel(c as u32, r);
            reds[c] = p[0] as f64 / 255.0;
            greens[c] = p[1] as f64 / 255.0;
            blues[c] = p[2] as f64 / 255.0;
            rs += reds[c];
            gs += greens[c];
            bs += blues[c];
        }
        let w = (width.max(1)) as f64;
        let (ra, ga, ba) = (rs / w, gs / w, bs / w);

        let mut distance = 0.0f64;
        for c in 0..width as usize {
            if distance > fuzz {
                break;
            }
            let d = (reds[c] - ra).powi(2) + (greens[c] - ga).powi(2) + (blues[c] - ba).powi(2);
            distance = distance.max(d);
        }
        if distance > fuzz {
            break;
        }

        if idx == 0 {
            first_avg = (ra, ga, ba);
        } else {
            let d = (first_avg.0 - ra).powi(2) + (first_avg.1 - ga).powi(2) + (first_avg.2 - ba).powi(2);
            if d > fuzz {
                break;
            }
        }
        ans += 1;
    }
    ans
}

/// Port of `remove_borders` (auto-trim): crops away homogeneous,
/// same-colored borders from all 4 edges, within `fuzz` (0-255,
/// matching upstream's own scale).
pub fn remove_borders(image: &RgbaImage, fuzz: f64) -> RgbaImage {
    let fuzz = fuzz / 255.0;
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return image.clone();
    }

    let top_border = read_border_row(image, fuzz, true);
    if top_border >= height.saturating_sub(1) {
        return image.clone();
    }
    let bottom_border = read_border_row(image, fuzz, false);
    if bottom_border >= height.saturating_sub(1) {
        return image.clone();
    }

    let rotated = image::imageops::rotate90(image);
    let left_border = read_border_row(&rotated, fuzz, true);
    if left_border >= width.saturating_sub(1) {
        return image.clone();
    }
    let right_border = read_border_row(&rotated, fuzz, false);
    if right_border >= width.saturating_sub(1) {
        return image.clone();
    }

    if left_border == 0 && right_border == 0 && top_border == 0 && bottom_border == 0 {
        return image.clone();
    }
    let new_w = width - left_border - right_border;
    let new_h = height - top_border - bottom_border;
    image::imageops::crop_imm(image, left_border, top_border, new_w, new_h).to_image()
}

/// Port of `BYTE_MUL` for a single channel byte: `round(x * a / 255)`
/// via the standard fast integer approximation.
fn byte_mul_channel(x: u8, a: u8) -> u8 {
    let t = x as u32 * a as u32;
    (((t + 128) + ((t + 128) >> 8)) >> 8) as u8
}

fn is_fully_transparent_black(p: &Rgba<u8>) -> bool {
    p[0] == 0 && p[1] == 0 && p[2] == 0 && p[3] == 0
}

/// Port of the real per-pixel blend both `overlay` and `texture_image`
/// use (`qt_blend_argb32_on_argb32`'s "source over opaque destination"
/// case): `dest = src_premultiplied + dest * (1 - alpha(src))`. Only
/// `src`'s R/G/B channels get premultiplied by its own alpha -- a
/// premultiplied pixel's ALPHA channel is the alpha value itself, not
/// alpha-multiplied-by-itself. `dest` (always opaque going in) stays
/// opaque coming out, since `alpha(src) + (255-alpha(src)) == 255`
/// (subject to the same rounding the fast integer divide-by-255 trick
/// uses elsewhere in this module).
fn blend_over_opaque(dest: &mut Rgba<u8>, src: &Rgba<u8>) {
    let a = src[3];
    let inv_alpha = 255 - a;
    for ch_idx in 0..3 {
        let src_premul = byte_mul_channel(src[ch_idx], a);
        let dest_blend = byte_mul_channel(dest[ch_idx], inv_alpha);
        dest[ch_idx] = src_premul.saturating_add(dest_blend);
    }
    let dest_alpha_blend = byte_mul_channel(dest[3], inv_alpha);
    dest[3] = a.saturating_add(dest_alpha_blend);
}

/// Port of `overlay`: alpha-composites `image` onto `canvas` at
/// `(left, top)`, clipped to the canvas bounds. See the module doc's
/// disclosed narrowing on the real "canvas must be opaque" precondition.
pub fn overlay(canvas: &mut RgbaImage, image: &RgbaImage, left: u32, top: u32) -> Result<(), String> {
    let (cw, ch) = canvas.dimensions();
    if cw < 1 || ch < 1 {
        return Err("The canvas cannot be a null image".to_string());
    }
    let (iw, ih) = image.dimensions();

    let left = left.min(cw - 1);
    let top = top.min(ch - 1);
    let right = (left + iw).min(cw);
    let bottom = (top + ih).min(ch);
    if right <= left || bottom <= top {
        return Ok(());
    }
    let width = right - left;
    let height = bottom - top;

    let has_alpha = image.pixels().any(|p| p[3] != 255);

    for r in 0..height {
        for c in 0..width {
            let src = *image.get_pixel(c, r);
            if !has_alpha {
                *canvas.get_pixel_mut(left + c, top + r) = src;
                continue;
            }
            let a = src[3];
            if a == 255 {
                *canvas.get_pixel_mut(left + c, top + r) = src;
            } else if !is_fully_transparent_black(&src) {
                let dest = canvas.get_pixel_mut(left + c, top + r);
                blend_over_opaque(dest, &src);
            }
        }
    }
    Ok(())
}

/// Port of `has_transparent_pixels`.
pub fn has_transparent_pixels(image: &RgbaImage) -> bool {
    image.pixels().any(|p| p[3] != 255)
}

/// Port of `set_opacity`: scales every pixel's alpha by `alpha`
/// (0.0-1.0), matching upstream's real truncating (not rounding)
/// `double`-to-`int` conversion.
pub fn set_opacity(image: &RgbaImage, alpha: f64) -> RgbaImage {
    let mut out = image.clone();
    for p in out.pixels_mut() {
        let new_alpha = (p[3] as f64 * alpha) as i64;
        p[3] = new_alpha.clamp(0, 255) as u8;
    }
    out
}

/// Port of `texture_image`: tiles `texture` across `canvas`, either
/// overwriting (if the texture itself has no transparency) or
/// alpha-blending (the same real blend as [`overlay`]) otherwise.
pub fn texture_image(canvas: &RgbaImage, texture: &RgbaImage) -> RgbaImage {
    let mut out = canvas.clone();
    let (cw, ch) = out.dimensions();
    let (tw, th) = texture.dimensions();
    if tw == 0 || th == 0 {
        return out;
    }
    let overwrite = !texture.pixels().any(|p| p[3] != 255);

    let mut y = 0u32;
    while y < ch {
        let ylimit = th.min(ch - y);
        let mut x = 0u32;
        while x < cw {
            let xlimit = tw.min(cw - x);
            for r in 0..ylimit {
                for c in 0..xlimit {
                    let src = *texture.get_pixel(c, r);
                    if overwrite {
                        *out.get_pixel_mut(x + c, y + r) = src;
                        continue;
                    }
                    let a = src[3];
                    if a == 255 {
                        *out.get_pixel_mut(x + c, y + r) = src;
                    } else if !is_fully_transparent_black(&src) {
                        let dest = out.get_pixel_mut(x + c, y + r);
                        blend_over_opaque(dest, &src);
                    }
                }
            }
            x += tw;
        }
        y += th;
    }
    out
}

/// HSV saturation (0.0-1.0), matching Qt's own `QColor::saturationF`.
fn hsv_saturation(rgb: [u8; 3]) -> f64 {
    let max = *rgb.iter().max().unwrap() as f64;
    let min = *rgb.iter().min().unwrap() as f64;
    if max == 0.0 {
        0.0
    } else {
        (max - min) / max
    }
}

/// Port of `dominant_color`: the most common color (quantized to 32
/// levels per channel for grouping), preferring a more vibrant
/// alternative among the top 5 if the winner is nearly grayscale.
/// Returns `None` for a null (zero-sized) image, matching real
/// upstream's `QColor()` (an invalid color) return.
pub fn dominant_color(image: &RgbaImage) -> Option<Rgba<u8>> {
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    let mut counts: HashMap<[u8; 3], u32> = HashMap::new();
    for p in image.pixels() {
        let q = [(p[0] / 8) * 8, (p[1] / 8) * 8, (p[2] / 8) * 8];
        *counts.entry(q).or_insert(0) += 1;
    }
    if counts.is_empty() {
        return None;
    }

    let mut sorted: Vec<([u8; 3], u32)> = counts.into_iter().collect();
    sorted.sort_by(|a, b| {
        if a.1 != b.1 {
            b.1.cmp(&a.1)
        } else {
            hsv_saturation(b.0).partial_cmp(&hsv_saturation(a.0)).unwrap_or(std::cmp::Ordering::Equal)
        }
    });

    let limit = 5.min(sorted.len());
    let mut ans = sorted[0].0;
    let saturation = hsv_saturation(ans);
    if saturation < 0.2 && sorted.len() > 1 {
        let min_num_pixels = (0.05 * width as f64 * height as f64) as u32;
        for &(color, count) in sorted.iter().take(limit).skip(1) {
            let q = hsv_saturation(color);
            if q > 0.3 && count > min_num_pixels {
                ans = color;
                break;
            }
        }
    }
    Some(Rgba([ans[0], ans[1], ans[2], 255]))
}

/// Port of `normalize`: per-channel histogram-stretch contrast
/// normalization, clipping at the 0.1% intensity levels. A no-op if
/// the image has fewer than 2 distinct colors.
pub fn normalize(image: &RgbaImage) -> RgbaImage {
    let mut img = image.clone();
    let (width, height) = img.dimensions();
    let count = width as i64 * height as i64;

    let mut hist_r = [0i64; 256];
    let mut hist_g = [0i64; 256];
    let mut hist_b = [0i64; 256];
    let mut distinct: HashSet<[u8; 4]> = HashSet::new();

    for p in img.pixels() {
        distinct.insert(p.0);
        hist_r[p[0] as usize] += 1;
        hist_g[p[1] as usize] += 1;
        hist_b[p[2] as usize] += 1;
    }
    if distinct.len() < 2 {
        return img;
    }

    let threshold = count / 1000;

    fn find_low(hist: &[i64; 256], start: u16, end: u16, threshold: i64) -> u16 {
        let mut acc = 0i64;
        let mut v = start;
        while v < end {
            acc += hist[v as usize];
            if acc > threshold {
                break;
            }
            v += 1;
        }
        v
    }
    fn find_high(hist: &[i64; 256], start: u16, end: u16, threshold: i64) -> u16 {
        let mut acc = 0i64;
        let mut v = start;
        while v > end {
            acc += hist[(v - 1) as usize];
            if acc > threshold {
                break;
            }
            v -= 1;
        }
        v
    }

    // Real upstream cascade: each channel's search range is bounded by
    // the PREVIOUS channel's already-found [low, high) range -- not
    // independent per-channel histograms. Reproduced exactly.
    let low_r = find_low(&hist_r, 0, 256, threshold);
    let high_r = find_high(&hist_r, 256, 0, threshold);
    let low_g = find_low(&hist_g, low_r, high_r, threshold);
    let high_g = find_high(&hist_g, high_r, low_r, threshold);
    let low_b = find_low(&hist_b, low_g, high_g, threshold);
    let high_b = find_high(&hist_b, high_g, low_g, threshold);

    let build_map = |low: u16, high: u16| -> [u8; 256] {
        let mut map = [0u8; 256];
        for i in 0..256u16 {
            if i < low {
                map[i as usize] = 0;
            } else if i > high {
                map[i as usize] = 255;
            } else if low != high {
                map[i as usize] = ((255 * (i - low) as i32) / (high - low) as i32) as u8;
            }
            // else: i == low == high, stays at the default 0 -- see the
            // module doc's disclosed narrowing.
        }
        map
    };

    let map_r = build_map(low_r, high_r);
    let map_g = build_map(low_g, high_g);
    let map_b = build_map(low_b, high_b);

    for p in img.pixels_mut() {
        if low_r != high_r {
            p[0] = map_r[p[0] as usize];
        }
        if low_g != high_g {
            p[1] = map_g[p[1] as usize];
        }
        if low_b != high_b {
            p[2] = map_b[p[2] as usize];
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba(rgba))
    }

    #[test]
    fn grayscale_converts_a_pure_red_pixel_via_the_real_qgray_weights() {
        let img = solid(2, 2, [255, 0, 0, 255]);
        let out = grayscale(&img);
        // qGray(255,0,0) = (255*11)/32 = 87
        assert_eq!(out.get_pixel(0, 0).0, [87, 87, 87, 255]);
    }

    #[test]
    fn remove_borders_crops_a_uniform_border() {
        let mut img = solid(10, 10, [255, 255, 255, 255]);
        for y in 3..7 {
            for x in 3..7 {
                img.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        let out = remove_borders(&img, 10.0);
        assert_eq!(out.dimensions(), (4, 4));
        assert_eq!(out.get_pixel(0, 0).0, [0, 0, 0, 255]);
    }

    #[test]
    fn remove_borders_is_a_no_op_on_an_already_tight_image() {
        let img = solid(4, 4, [0, 0, 0, 255]);
        let out = remove_borders(&img, 10.0);
        assert_eq!(out.dimensions(), (4, 4));
    }

    #[test]
    fn overlay_blends_a_semi_transparent_image_onto_an_opaque_canvas() {
        let mut canvas = solid(4, 4, [255, 255, 255, 255]);
        let src = solid(2, 2, [0, 0, 0, 128]);
        overlay(&mut canvas, &src, 1, 1).unwrap();
        // Roughly half-blended toward black; not pure white anymore,
        // canvas stays opaque.
        let p = canvas.get_pixel(1, 1);
        assert!(p[0] < 255 && p[0] > 0);
        assert_eq!(p[3], 255);
        // Untouched corner stays pure white.
        assert_eq!(canvas.get_pixel(0, 0).0, [255, 255, 255, 255]);
    }

    #[test]
    fn overlay_clips_to_canvas_bounds() {
        let mut canvas = solid(2, 2, [255, 255, 255, 255]);
        let src = solid(4, 4, [0, 0, 0, 255]);
        overlay(&mut canvas, &src, 1, 1).unwrap();
        // Only the (1,1) pixel should be overwritten; nothing panics
        // from the out-of-bounds portion of `src`.
        assert_eq!(canvas.get_pixel(1, 1).0, [0, 0, 0, 255]);
        assert_eq!(canvas.get_pixel(0, 0).0, [255, 255, 255, 255]);
    }

    #[test]
    fn has_transparent_pixels_detects_any_non_opaque_pixel() {
        let opaque = solid(2, 2, [1, 2, 3, 255]);
        assert!(!has_transparent_pixels(&opaque));
        let mut transparent = opaque.clone();
        transparent.put_pixel(0, 0, Rgba([1, 2, 3, 254]));
        assert!(has_transparent_pixels(&transparent));
    }

    #[test]
    fn set_opacity_scales_alpha_and_truncates() {
        let img = solid(1, 1, [10, 20, 30, 200]);
        let out = set_opacity(&img, 0.5);
        assert_eq!(out.get_pixel(0, 0)[3], 100);
    }

    #[test]
    fn texture_image_tiles_an_opaque_texture_across_a_larger_canvas() {
        let canvas = solid(4, 4, [255, 255, 255, 255]);
        let texture = solid(2, 2, [0, 0, 0, 255]);
        let out = texture_image(&canvas, &texture);
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(out.get_pixel(x, y).0, [0, 0, 0, 255]);
            }
        }
    }

    #[test]
    fn dominant_color_picks_the_most_common_color() {
        let mut img = solid(10, 10, [0, 0, 255, 255]); // mostly blue
        for x in 0..3 {
            img.put_pixel(x, 0, Rgba([255, 0, 0, 255])); // a few red pixels
        }
        let dom = dominant_color(&img).unwrap();
        // Real upstream quantizes to 32 levels per channel (`(v/8)*8`)
        // before counting, so 255 buckets down to 248, not 255.
        assert_eq!(dom.0, [0, 0, 248, 255]);
    }

    #[test]
    fn dominant_color_is_none_for_a_zero_sized_image() {
        let img = RgbaImage::new(0, 0);
        assert!(dominant_color(&img).is_none());
    }

    #[test]
    fn normalize_stretches_a_low_contrast_image() {
        // Two distinct mid-gray shades, close together -- should
        // stretch toward the full 0-255 range.
        let mut img = solid(10, 10, [100, 100, 100, 255]);
        for x in 0..5 {
            img.put_pixel(x, 0, Rgba([120, 120, 120, 255]));
        }
        let out = normalize(&img);
        let lo = out.get_pixel(9, 9)[0];
        let hi = out.get_pixel(0, 0)[0];
        assert!(hi > lo, "the higher input intensity should map to a higher output intensity");
        assert!((hi as i32 - lo as i32) > (120 - 100), "contrast should have increased");
    }

    #[test]
    fn normalize_is_a_no_op_for_a_single_color_image() {
        let img = solid(4, 4, [50, 60, 70, 255]);
        let out = normalize(&img);
        assert_eq!(out, img);
    }
}
