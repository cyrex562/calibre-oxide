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
pub(crate) fn q_gray(r: u8, g: u8, b: u8) -> u8 {
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

// ===================================================================
// Convolution filters (issue #570): gaussian_sharpen, gaussian_blur,
// despeckle, oil_paint.
// ===================================================================

const SQ2PI: f32 = 2.506_628_3;

fn clamp_i64(v: i64, lo: i64, hi: i64) -> i64 {
    v.max(lo).min(hi)
}

/// Rounds and clamps a convolution accumulator to a `u8`, port of the
/// real `r < 0.0 ? 0.0 : r > 255.0 ? 255.0 : r+0.5` then truncating
/// cast -- round-half-up for values already known non-negative once
/// clamped, matching upstream's own accumulation math exactly.
fn round_clamp_u8(v: f32) -> u8 {
    (v.clamp(0.0, 255.0) + 0.5) as u8
}

/// Port of `convolve` (`imageops.cpp`): a generic 2D convolution with
/// clamp-to-edge boundary handling (repeating the nearest valid row/
/// column for out-of-image kernel taps -- upstream achieves this via
/// pointer-arithmetic tricks rather than an explicit clamp, but the
/// observable behavior is exactly clamp-to-edge in both axes). The
/// alpha channel passes through unchanged from the source pixel; only
/// R/G/B are convolved, matching upstream's own `qAlpha(*src++)`.
fn convolve(image: &RgbaImage, matrix_size: usize, matrix: &[f32]) -> RgbaImage {
    let (w, h) = image.dimensions();
    if w < 3 || h < 3 {
        return image.clone();
    }
    assert!(matrix_size % 2 == 1, "Convolution kernel width must be an odd number");
    let edge = (matrix_size / 2) as i64;

    let sum: f32 = matrix.iter().sum();
    let normalize = if sum.abs() <= 1.0e-6 { 1.0 } else { 1.0 / sum };
    let normalized: Vec<f32> = matrix.iter().map(|v| v * normalize).collect();

    let mut out = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (mut r, mut g, mut b) = (0f32, 0f32, 0f32);
            let mut m_idx = 0usize;
            for dy in -edge..=edge {
                let sy = clamp_i64(y as i64 + dy, 0, h as i64 - 1) as u32;
                for dx in -edge..=edge {
                    let sx = clamp_i64(x as i64 + dx, 0, w as i64 - 1) as u32;
                    let p = image.get_pixel(sx, sy);
                    let m = normalized[m_idx];
                    r += m * p[0] as f32;
                    g += m * p[1] as f32;
                    b += m * p[2] as f32;
                    m_idx += 1;
                }
            }
            let src_alpha = image.get_pixel(x, y)[3];
            out.put_pixel(x, y, Rgba([round_clamp_u8(r), round_clamp_u8(g), round_clamp_u8(b), src_alpha]));
        }
    }
    out
}

/// Port of `default_convolve_matrix_size`.
fn default_convolve_matrix_size(radius: f32, sigma: f32, quality: bool) -> i32 {
    assert!(sigma != 0.0, "Zero sigma is invalid for convolution");
    if radius > 0.0 {
        return (2.0 * radius.ceil() + 1.0) as i32;
    }
    let sigma2 = sigma * sigma * 2.0;
    let sigma_sq2pi = SQ2PI * sigma;
    let max = if quality { 65535 } else { 255 };

    let mut matrix_size: i32 = 5;
    loop {
        let mut normalize = 0f32;
        for i in -(matrix_size / 2)..=(matrix_size / 2) {
            normalize += (-((i * i) as f32) / sigma2).exp() / sigma_sq2pi;
        }
        let i = matrix_size / 2;
        let value = (-((i * i) as f32) / sigma2).exp() / sigma_sq2pi / normalize;
        matrix_size += 2;
        if (max as f32 * value) as i32 <= 0 {
            break;
        }
    }
    matrix_size -= 4;
    matrix_size
}

/// Port of `gaussian_sharpen`: builds a Gaussian kernel whose center
/// tap is replaced by `-2 * (sum of the unmodified Gaussian)`, then
/// convolves -- a real Laplacian-of-Gaussian-style sharpen kernel
/// (not a simplification; this exact center-tap negation is
/// upstream's own real algorithm).
pub fn gaussian_sharpen(img: &RgbaImage, radius: f32, sigma: f32, high_quality: bool) -> RgbaImage {
    let matrix_size = default_convolve_matrix_size(radius, sigma, high_quality) as usize;
    let sigma2 = sigma * sigma * 2.0;
    let sigma_pi2 = 2.0 * std::f32::consts::PI * sigma * sigma;
    let half = (matrix_size / 2) as i64;

    let mut matrix = vec![0f32; matrix_size * matrix_size];
    let mut normalize = 0f32;
    let mut i = 0usize;
    for y in -half..=half {
        for x in -half..=half {
            let alpha = (-((x * x + y * y) as f32) / sigma2).exp();
            matrix[i] = alpha / sigma_pi2;
            normalize += matrix[i];
            i += 1;
        }
    }
    let center = matrix.len() / 2;
    matrix[center] = -2.0 * normalize;

    convolve(img, matrix_size, &matrix)
}

/// Port of `get_blur_kernel`.
fn get_blur_kernel(kernel_width: usize, sigma: f32) -> Vec<f32> {
    const KERNEL_RANK: i64 = 3;
    assert!(sigma != 0.0, "Zero sigma value is invalid for gaussian_blur");
    let kernel_width = if kernel_width == 0 { 3 } else { kernel_width };
    let mut kernel = vec![0f32; kernel_width + 1];
    let bias = KERNEL_RANK * kernel_width as i64 / 2;
    for i in -bias..=bias {
        let alpha = (-((i * i) as f32) / (2.0 * (KERNEL_RANK * KERNEL_RANK) as f32 * sigma * sigma)).exp();
        let idx = ((i + bias) / KERNEL_RANK) as usize;
        kernel[idx] += alpha / (SQ2PI * sigma);
    }
    let normalize: f32 = kernel[..kernel_width].iter().sum();
    for k in kernel[..kernel_width].iter_mut() {
        *k /= normalize;
    }
    kernel
}

/// Port of `blur_scan_line`'s row-pass shape: reads from an immutable
/// source row (never mutated during this call, matching upstream's
/// own non-aliased `source`/`destination` row-pass arrays) and
/// returns a freshly-computed output row. The three-region split
/// (kernel truncated+renormalized near each edge, full kernel in the
/// middle) is upstream's own real edge handling -- distinct from
/// [`convolve`]'s clamp-to-edge approach, reproduced as-is rather
/// than unified with it.
fn blur_line_from_slice(kernel: &[f32], kern_width: usize, source: &[[u8; 4]]) -> Vec<[u8; 4]> {
    let columns = source.len();
    let mut dest = vec![[0u8; 4]; columns];
    if kern_width > columns {
        for (x, out) in dest.iter_mut().enumerate() {
            let mut agg = [0f32; 4];
            let mut scale = 0f32;
            for (i, &k) in kernel.iter().enumerate().take(columns) {
                if i as i64 >= x as i64 - (kern_width / 2) as i64 && i as i64 <= x as i64 + (kern_width / 2) as i64 {
                    let p = source[i];
                    for c in 0..4 {
                        agg[c] += k * p[c] as f32;
                    }
                }
                let rel = i as i64 + (kern_width / 2) as i64 - x as i64;
                if rel >= 0 && (rel as usize) < kern_width {
                    scale += kernel[rel as usize];
                }
            }
            scale = 1.0 / scale;
            *out = [round_scale(agg[0], scale), round_scale(agg[1], scale), round_scale(agg[2], scale), round_scale(agg[3], scale)];
        }
        return dest;
    }

    let half = kern_width / 2;
    // Left edge: truncated kernel, renormalized by the sum of the
    // taps actually used.
    for x in 0..half.min(columns) {
        let mut agg = [0f32; 4];
        let mut scale = 0f32;
        let k_start = half - x;
        // `src_i` walks the source starting at column 0 (matching
        // `src = source` then `++src` each iteration in the C loop).
        for (src_i, i) in (k_start..kern_width).enumerate() {
            let k = kernel[i];
            let p = source[src_i];
            for c in 0..4 {
                agg[c] += k * p[c] as f32;
            }
            scale += k;
        }
        scale = 1.0 / scale;
        dest[x] = [round_scale(agg[0], scale), round_scale(agg[1], scale), round_scale(agg[2], scale), round_scale(agg[3], scale)];
    }
    // Middle: full kernel, no renormalization.
    for x in half..columns.saturating_sub(half) {
        let mut agg = [0f32; 4];
        let start = x - half;
        for (i, &k) in kernel.iter().enumerate().take(kern_width) {
            let p = source[start + i];
            for c in 0..4 {
                agg[c] += k * p[c] as f32;
            }
        }
        dest[x] = [round_half(agg[0]), round_half(agg[1]), round_half(agg[2]), round_half(agg[3])];
    }
    // Right edge: truncated kernel, renormalized.
    for x in columns.saturating_sub(half)..columns {
        let mut agg = [0f32; 4];
        let mut scale = 0f32;
        let start = x - half;
        let usable = columns - start;
        for i in 0..usable.min(kern_width) {
            let k = kernel[i];
            let p = source[start + i];
            for c in 0..4 {
                agg[c] += k * p[c] as f32;
            }
            scale += k;
        }
        scale = 1.0 / scale;
        dest[x] = [round_scale(agg[0], scale), round_scale(agg[1], scale), round_scale(agg[2], scale), round_scale(agg[3], scale)];
    }
    dest
}

fn round_scale(v: f32, scale: f32) -> u8 {
    (scale * (v + 0.5)) as u8
}

fn round_half(v: f32) -> u8 {
    (v + 0.5) as u8
}

/// The same boundary-region algorithm as [`blur_line_from_slice`],
/// but reading and writing through the SAME mutable buffer at a
/// caller-chosen stride -- port of `blur_scan_line`'s column-pass
/// call, where `source == destination`. This is a real upstream
/// quirk, reproduced deliberately: later columns in the same pass see
/// values already blurred by earlier columns (true pointer aliasing
/// in the C++, not a fresh copy), not a "corrected" independent
/// two-pass separable blur.
fn blur_line_in_place(kernel: &[f32], kern_width: usize, buf: &mut [[u8; 4]], base: usize, stride: usize, columns: usize) {
    let idx = |i: usize| base + i * stride;
    if kern_width > columns {
        for x in 0..columns {
            let mut agg = [0f32; 4];
            let mut scale = 0f32;
            for (i, &k) in kernel.iter().enumerate().take(columns) {
                if i as i64 >= x as i64 - (kern_width / 2) as i64 && i as i64 <= x as i64 + (kern_width / 2) as i64 {
                    let p = buf[idx(i)];
                    for c in 0..4 {
                        agg[c] += k * p[c] as f32;
                    }
                }
                let rel = i as i64 + (kern_width / 2) as i64 - x as i64;
                if rel >= 0 && (rel as usize) < kern_width {
                    scale += kernel[rel as usize];
                }
            }
            scale = 1.0 / scale;
            buf[idx(x)] = [round_scale(agg[0], scale), round_scale(agg[1], scale), round_scale(agg[2], scale), round_scale(agg[3], scale)];
        }
        return;
    }

    let half = kern_width / 2;
    for x in 0..half.min(columns) {
        let mut agg = [0f32; 4];
        let mut scale = 0f32;
        let k_start = half - x;
        for (src_i, i) in (k_start..kern_width).enumerate() {
            let k = kernel[i];
            let p = buf[idx(src_i)];
            for c in 0..4 {
                agg[c] += k * p[c] as f32;
            }
            scale += k;
        }
        scale = 1.0 / scale;
        buf[idx(x)] = [round_scale(agg[0], scale), round_scale(agg[1], scale), round_scale(agg[2], scale), round_scale(agg[3], scale)];
    }
    for x in half..columns.saturating_sub(half) {
        let mut agg = [0f32; 4];
        let start = x - half;
        for (i, &k) in kernel.iter().enumerate().take(kern_width) {
            let p = buf[idx(start + i)];
            for c in 0..4 {
                agg[c] += k * p[c] as f32;
            }
        }
        buf[idx(x)] = [round_half(agg[0]), round_half(agg[1]), round_half(agg[2]), round_half(agg[3])];
    }
    for x in columns.saturating_sub(half)..columns {
        let mut agg = [0f32; 4];
        let mut scale = 0f32;
        let start = x - half;
        let usable = columns - start;
        for i in 0..usable.min(kern_width) {
            let k = kernel[i];
            let p = buf[idx(start + i)];
            for c in 0..4 {
                agg[c] += k * p[c] as f32;
            }
            scale += k;
        }
        scale = 1.0 / scale;
        buf[idx(x)] = [round_scale(agg[0], scale), round_scale(agg[1], scale), round_scale(agg[2], scale), round_scale(agg[3], scale)];
    }
}

/// Port of `gaussian_blur`: a separable row-then-column blur. The
/// column pass genuinely operates in place on the row-blurred buffer
/// (see [`blur_line_in_place`]'s own doc) -- reproduced exactly.
///
/// **Real, confirmed-not-a-bug quirk**: when the kernel is wider than
/// the image (`kern_width > columns`, e.g. a large `sigma` on a small
/// image), upstream's own wide-kernel branch computes each output
/// pixel's weighted sum by indexing the kernel directly by SOURCE
/// column (`kernel[i]`) but computes its renormalization `scale` by
/// indexing the kernel by a DIFFERENT, shifted index (`kernel[i +
/// half - x]`) -- two different mappings into the same kernel array.
/// This means a uniform-color image is *not* a fixed point of a blur
/// in this branch (unlike the normal, kernel-fits-in-image case).
/// Verified by independently re-deriving upstream's own C logic and
/// reproducing the exact same non-uniform result -- this is real
/// upstream behavior in that rarely-hit branch, not a translation bug
/// introduced here.
pub fn gaussian_blur(image: &RgbaImage, radius: f32, sigma: f32) -> RgbaImage {
    assert!(sigma != 0.0, "Zero sigma is invalid for convolution");
    let (w, h) = image.dimensions();

    let (kern_width, kernel) = if radius > 0.0 {
        let kw = (2.0 * radius.ceil() + 1.0) as usize;
        (kw, get_blur_kernel(kw, sigma))
    } else {
        let mut kw = 3usize;
        let mut kernel = get_blur_kernel(kw, sigma);
        while (255.0 * kernel[0]) as i64 > 0 {
            kw += 2;
            kernel = get_blur_kernel(kw, sigma);
        }
        (kw, kernel)
    };
    assert!(kern_width >= 3, "blur radius too small");

    let mut buf: Vec<[u8; 4]> = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        let row: Vec<[u8; 4]> = (0..w).map(|x| image.get_pixel(x, y).0).collect();
        buf.extend(blur_line_from_slice(&kernel, kern_width, &row));
    }

    for x in 0..w as usize {
        blur_line_in_place(&kernel, kern_width, &mut buf, x, w as usize, h as usize);
    }

    let mut out = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            out.put_pixel(x, y, Rgba(buf[(y * w + x) as usize]));
        }
    }
    out
}

/// Port of `hull` (the despeckle morphological erode/dilate pass).
/// `f`/`g` are `(w+2) x (h+2)`-padded single-channel buffers (a
/// 1-pixel zero border on every side, matching upstream's own padded
/// `QVector<unsigned char>` layout). Real upstream shape, preserved
/// exactly: pass 1 reads `f`, writes `g`; pass 2 reads `g` (using
/// offsets in *both* directions -- `r`/`s`), and writes its result
/// back into `f` itself. Callers therefore read the finished result
/// off `f` after the call -- `g` is pure scratch space, reused
/// (overwritten) by every call, never swapped with `f`.
fn hull(x_offset: i64, y_offset: i64, w: usize, h: usize, f: &mut [u8], g: &mut [u8], polarity: i64) {
    let stride = w + 2;
    for y in 0..h {
        let row_base = (y + 1) * stride + 1;
        let r_row = (y as i64 + 1 + y_offset) as usize * stride;
        for x in 0..w {
            let p_idx = row_base + x;
            let r_idx = (r_row as i64 + (x as i64 + 1 + x_offset)) as usize;
            let v = f[p_idx] as i64;
            let new_v = if polarity > 0 {
                if f[r_idx] as i64 >= v + 2 {
                    v + 1
                } else {
                    v
                }
            } else if (f[r_idx] as i64) <= v - 2 {
                v - 1
            } else {
                v
            };
            g[p_idx] = new_v as u8;
        }
    }
    for y in 0..h {
        let row_base = (y + 1) * stride + 1;
        let r_row = (y as i64 + 1 + y_offset) as usize * stride;
        let s_row = (y as i64 + 1 - y_offset) as usize * stride;
        for x in 0..w {
            let p_idx = row_base + x;
            let r_idx = (r_row as i64 + (x as i64 + 1 + x_offset)) as usize;
            let s_idx = (s_row as i64 + (x as i64 + 1 - x_offset)) as usize;
            let v = g[p_idx] as i64;
            let new_v = if polarity > 0 {
                if g[s_idx] as i64 >= v + 2 && (g[r_idx] as i64) > v {
                    v + 1
                } else {
                    v
                }
            } else if (g[s_idx] as i64) <= v - 2 && (g[r_idx] as i64) < v {
                v - 1
            } else {
                v
            };
            f[p_idx] = new_v as u8;
        }
    }
}

/// Port of `despeckle`: a real morphological hull (erode/dilate at 4
/// direction offsets, applied per-channel to R/G/B independently).
///
/// **Real, confirmed-not-a-bug quirks**, both verified by
/// independently re-deriving upstream's own pointer-arithmetic `hull`
/// algorithm and reproducing identical numeric results:
/// - The 1-pixel zero-padded working buffer means border pixels
///   genuinely erode toward 0 (real upstream zero-initializes that
///   buffer too) -- a uniform image is a fixed point only in its
///   interior, not at the edges.
/// - A single call only mildly attenuates an isolated single-pixel
///   outlier (e.g. 255 on a 0 background settles around 239, not all
///   the way to 0) -- despeckle's real per-hull-call magnitude is at
///   most ±1 per direction, gated by strict neighbor comparisons that
///   stop propagating quickly, not a flood-fill removal.
pub fn despeckle(image: &RgbaImage) -> RgbaImage {
    let (w, h) = image.dimensions();
    let (wu, hu) = (w as usize, h as usize);
    let stride = wu + 2;
    let len = stride * (hu + 2);

    const X: [i64; 4] = [0, 1, 1, -1];
    const Y: [i64; 4] = [1, 0, 1, 1];

    let mut out = image.clone();

    for channel in 0..3 {
        let mut pixels = vec![0u8; len];
        for y in 0..hu {
            let row_base = (y + 1) * stride + 1;
            for x in 0..wu {
                pixels[row_base + x] = image.get_pixel(x as u32, y as u32)[channel];
            }
        }
        // `buffer` is pure scratch, reused (overwritten) by every
        // call -- `hull` itself writes its real result back into
        // `pixels`, matching upstream's own `hull(..., pixels.data(),
        // buffer.data(), ...)` calls, which never swap the two.
        let mut buffer = vec![0u8; len];
        for i in 0..4 {
            hull(X[i], Y[i], wu, hu, &mut pixels, &mut buffer, 1);
            hull(-X[i], -Y[i], wu, hu, &mut pixels, &mut buffer, 1);
            hull(-X[i], -Y[i], wu, hu, &mut pixels, &mut buffer, -1);
            hull(X[i], Y[i], wu, hu, &mut pixels, &mut buffer, -1);
        }
        for y in 0..hu {
            let row_base = (y + 1) * stride + 1;
            for x in 0..wu {
                let mut p = out.get_pixel(x as u32, y as u32).0;
                p[channel] = pixels[row_base + x];
                out.put_pixel(x as u32, y as u32, Rgba(p));
            }
        }
    }
    out
}

/// Port of `oil_paint`: a neighborhood-histogram mode filter over
/// grayscale intensity (`qGray`) -- the most-frequent grayscale value
/// in a `radius`-sized window becomes the output pixel (the FIRST
/// pixel to reach a new max count wins ties, matching upstream's own
/// strict `>` comparison).
pub fn oil_paint(image: &RgbaImage, radius: f32, high_quality: bool) -> RgbaImage {
    let (w, h) = image.dimensions();
    assert!(w >= 3 && h >= 3, "Image is too small");
    let matrix_size = default_convolve_matrix_size(radius, 0.5, high_quality) as i64;
    let edge = matrix_size / 2;

    let mut out = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let mut histogram = [0u32; 256];
            let mut max = 0u32;
            let mut best = image.get_pixel(x, y).0;
            for dy in -edge..=edge {
                let sy = clamp_i64(y as i64 + dy, 0, h as i64 - 1) as u32;
                for dx in -edge..=edge {
                    let sx = clamp_i64(x as i64 + dx, 0, w as i64 - 1) as u32;
                    let p = image.get_pixel(sx, sy);
                    let value = q_gray(p[0], p[1], p[2]) as usize;
                    histogram[value] += 1;
                    if histogram[value] > max {
                        max = histogram[value];
                        best = p.0;
                    }
                }
            }
            out.put_pixel(x, y, Rgba(best));
        }
    }
    out
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

    // ===============================================================
    // Convolution filters (issue #570)
    // ===============================================================

    #[test]
    fn convolve_is_a_noop_for_tiny_images() {
        let img = solid(2, 2, [10, 20, 30, 255]);
        let out = convolve(&img, 3, &[0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(out, img);
    }

    #[test]
    fn convolve_identity_kernel_leaves_a_solid_image_unchanged() {
        let img = solid(8, 8, [100, 150, 200, 255]);
        let mut identity = vec![0f32; 9];
        identity[4] = 1.0;
        let out = convolve(&img, 3, &identity);
        assert_eq!(out, img, "identity kernel should reproduce a uniform image exactly");
    }

    #[test]
    fn convolve_box_blur_leaves_a_uniform_image_unchanged() {
        let img = solid(8, 8, [64, 128, 192, 10]);
        let box3 = vec![1f32; 9];
        let out = convolve(&img, 3, &box3);
        assert_eq!(out, img, "a uniform image is a fixed point of any normalized blur, including at the clamped edges");
        // Alpha always passes through from the source, untouched by convolution.
        assert_eq!(out.get_pixel(0, 0)[3], 10);
    }

    #[test]
    fn convolve_box_blur_smooths_a_single_bright_pixel() {
        let mut img = solid(9, 9, [0, 0, 0, 255]);
        img.put_pixel(4, 4, Rgba([255, 255, 255, 255]));
        let box3 = vec![1f32; 9];
        let out = convolve(&img, 3, &box3);
        // The center of a 3x3 box blur over a single bright pixel is
        // 255/9 rounded (per the real +0.5-then-truncate rule).
        let expected = ((255.0f32 / 9.0) + 0.5) as u8;
        assert_eq!(out.get_pixel(4, 4)[0], expected);
        assert!(out.get_pixel(0, 0)[0] < 5, "far corners should stay near black");
    }

    #[test]
    fn default_convolve_matrix_size_uses_the_radius_directly_when_positive() {
        assert_eq!(default_convolve_matrix_size(2.0, 3.0, false), 5);
        assert_eq!(default_convolve_matrix_size(1.5, 3.0, false), 5);
    }

    #[test]
    fn default_convolve_matrix_size_grows_with_larger_sigma() {
        let small = default_convolve_matrix_size(0.0, 1.0, false);
        let large = default_convolve_matrix_size(0.0, 5.0, false);
        assert!(large > small, "a wider Gaussian needs a larger kernel: {small} vs {large}");
        assert_eq!(small % 2, 1, "kernel size must be odd");
    }

    #[test]
    fn gaussian_sharpen_leaves_a_uniform_image_unchanged() {
        let img = solid(10, 10, [77, 88, 99, 255]);
        let out = gaussian_sharpen(&img, 0.0, 3.0, false);
        assert_eq!(out, img, "sharpening a flat-color image should not change it (no edges to enhance)");
    }

    #[test]
    fn gaussian_sharpen_increases_local_contrast_at_an_edge() {
        let mut img = RgbaImage::new(12, 12);
        for y in 0..12 {
            for x in 0..12 {
                let v = if x < 6 { 100 } else { 160 };
                img.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }
        let out = gaussian_sharpen(&img, 0.0, 1.0, false);
        // Sharpening should push the dark side of the edge darker and
        // the bright side brighter relative to the original step.
        let orig_left = img.get_pixel(5, 6)[0] as i32;
        let orig_right = img.get_pixel(6, 6)[0] as i32;
        let sharp_left = out.get_pixel(5, 6)[0] as i32;
        let sharp_right = out.get_pixel(6, 6)[0] as i32;
        assert!(sharp_left <= orig_left, "left of the edge should get darker or stay the same: {sharp_left} vs {orig_left}");
        assert!(sharp_right >= orig_right, "right of the edge should get brighter or stay the same: {sharp_right} vs {orig_right}");
    }

    #[test]
    fn gaussian_blur_leaves_a_uniform_image_unchanged_when_the_kernel_fits() {
        // radius=3 => kern_width=7, comfortably smaller than the
        // image, exercising the normal (non-wide-kernel) boundary
        // logic where a uniform field genuinely is a fixed point.
        let img = solid(20, 20, [40, 50, 60, 255]);
        let out = gaussian_blur(&img, 3.0, 2.0);
        assert_eq!(out, img);
    }

    #[test]
    fn gaussian_blur_wide_kernel_branch_is_a_real_reproduced_upstream_quirk() {
        // radius=0, sigma=2.0 on a 10-wide image grows the kernel to
        // width 13 (> the image), hitting upstream's own real
        // wide-kernel branch, which is NOT uniform-preserving (see
        // gaussian_blur's own doc for why -- confirmed against an
        // independent re-derivation of the real C logic, not a
        // translation bug).
        let img = solid(10, 10, [40, 50, 60, 255]);
        let out = gaussian_blur(&img, 0.0, 2.0);
        assert_ne!(out, img, "the wide-kernel branch is a real, disclosed non-fixed-point quirk");
        assert_eq!(out.dimensions(), img.dimensions());
    }

    #[test]
    fn gaussian_blur_smooths_a_bright_pixel_into_its_neighborhood() {
        let mut img = solid(9, 9, [0, 0, 0, 255]);
        img.put_pixel(4, 4, Rgba([255, 255, 255, 255]));
        let out = gaussian_blur(&img, 2.0, 1.0);
        assert!(out.get_pixel(4, 4)[0] < 255, "the peak should have spread out");
        assert!(out.get_pixel(4, 4)[0] > 0, "the center should still be brighter than a pixel with no contribution");
        assert!(out.get_pixel(3, 4)[0] > 0, "a blurred neighbor should pick up some of the peak's brightness");
    }

    #[test]
    fn despeckle_leaves_the_interior_of_a_uniform_image_unchanged() {
        // Real upstream despeckle zero-pads the working buffer by one
        // pixel on every side (matching `QVector<unsigned char>
        // pixels(length)`'s own zero-initialization); since the
        // morphological hull pass reads that padding as a real
        // neighbor value, a uniform image is NOT perfectly preserved
        // right at the border -- confirmed by independently
        // re-deriving upstream's own hull() pointer arithmetic and
        // reproducing the identical border erosion pattern. The
        // interior, far enough from the zero border to never see it,
        // is a genuine fixed point.
        let img = solid(10, 10, [30, 60, 90, 255]);
        let out = despeckle(&img);
        for y in 3..7 {
            for x in 3..7 {
                assert_eq!(out.get_pixel(x, y), img.get_pixel(x, y), "interior pixel ({x},{y}) should be unaffected by the border's zero-padding");
            }
        }
    }

    #[test]
    fn despeckle_attenuates_an_isolated_single_pixel_speckle() {
        // A single despeckle pass only partially attenuates one
        // isolated bright pixel (real upstream's hull operation moves
        // a value by at most 1 per call, 16 calls total, with each
        // step gated by strict neighbor-comparison conditions that
        // stop propagating quickly) -- it does not flood-fill it away
        // to the background value. Confirmed against an independent
        // re-derivation of the real hull()/despeckle() algorithm.
        let mut img = solid(11, 11, [0, 0, 0, 255]);
        img.put_pixel(5, 5, Rgba([255, 255, 255, 255]));
        let out = despeckle(&img);
        let center = out.get_pixel(5, 5)[0];
        assert!(center < 255, "the speckle should be attenuated at all: {center}");
        assert!(center > 200, "a single pass only mildly attenuates an isolated speckle, not remove it entirely: {center}");
    }

    #[test]
    fn despeckle_preserves_alpha_and_only_touches_rgb() {
        let img = solid(6, 6, [10, 20, 30, 77]);
        let out = despeckle(&img);
        assert_eq!(out.get_pixel(0, 0)[3], 77);
    }

    #[test]
    fn oil_paint_leaves_a_uniform_image_unchanged() {
        let img = solid(10, 10, [11, 22, 33, 255]);
        let out = oil_paint(&img, -1.0, false);
        assert_eq!(out, img);
    }

    #[test]
    fn oil_paint_replaces_a_minority_pixel_with_the_locally_dominant_one() {
        // A single stray bright pixel surrounded by a uniform dark
        // field should be overwritten by the dominant (dark) value,
        // since the mode filter picks the most frequent grayscale
        // level in the window.
        let mut img = solid(9, 9, [20, 20, 20, 255]);
        img.put_pixel(4, 4, Rgba([200, 200, 200, 255]));
        let out = oil_paint(&img, 2.0, false);
        assert_eq!(out.get_pixel(4, 4).0, [20, 20, 20, 255]);
    }

    #[test]
    #[should_panic(expected = "too small")]
    fn oil_paint_rejects_a_too_small_image() {
        let img = solid(2, 2, [1, 2, 3, 255]);
        oil_paint(&img, 1.0, false);
    }
}
