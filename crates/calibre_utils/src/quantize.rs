//! Port of `calibre.utils.imageops.quantize`/`ordered_dither`
//! (`quantize.cpp`/`ordered_dither.cpp`, issue #571, the final piece
//! of the `utils/imageops` cluster, issue #67): real octree-based
//! color quantization with Floyd-Steinberg dithering, plus a real
//! ImageMagick-derived 8x8 ordered dither for e-ink output.
//!
//! # Scope
//!
//! [`quantize`] mirrors real `calibre.utils.img.quantize_image`'s own
//! wrapper behavior (not the raw C++ `imageops.quantize`'s stricter
//! precondition): if the image carries any non-opaque pixel, it's
//! first blended onto a white canvas via the already-real [`overlay`]
//! (matching upstream's own `blend_image`), since [`image::RgbaImage`]
//! always structurally carries an alpha channel the way Qt's
//! `Format_ARGB32` does -- a literal port of the raw C++ function's
//! `if (img.hasAlphaChannel()) throw ...` would reject every real
//! `RgbaImage` input unconditionally, which is not upstream's real
//! observable behavior for its own actual callers (all of whom go
//! through the Python wrapper that pre-blends first).
//!
//! **Disclosed, not ported**: the pre-quantized (`Format_Indexed8`)
//! input path (`read_colors(QVector<QRgb> color_table, ...)`) -- no
//! real caller in this crate ever constructs an already-indexed
//! image; every caller has a full RGB(A) image. The octree/dithering
//! algorithm itself (the real value of this port) is identical either
//! way; only the "read the palette instead of the pixels" entry point
//! is narrower.
//!
//! The octree is represented as a `Vec<Node>` arena (index-based
//! children/next-pointers) rather than upstream's own raw-pointer
//! memory pool -- this crate's own established pattern for tree
//! structures (see `calibre_ebooks::dom::Dom`), and a more natural fit
//! for Rust's ownership model than replicating a hand-rolled C++ pool
//! allocator.

use image::{Rgba, RgbaImage};

use crate::imageops::overlay;

const MAX_LEAVES: usize = 2000;
const MAX_DEPTH: usize = 8;
const MAX_COLORS: usize = 256;

fn get_index(r: u8, g: u8, b: u8, level: usize) -> usize {
    let shift = 7 - level;
    let br = (r >> shift) & 1;
    let bg = (g >> shift) & 1;
    let bb = (b >> shift) & 1;
    ((br << 2) | (bg << 1) | bb) as usize
}

fn euclidean_distance(r1: i64, g1: i64, b1: i64, r2: i64, g2: i64, b2: i64) -> i64 {
    (r1 * r1) + (r2 * r2) + (g1 * g1) + (g2 * g2) + (b1 * b1) + (b2 * b2) - 2 * (r1 * r2 + g1 * g2 + b1 * b2)
}

#[derive(Debug, Clone, Default)]
struct Node {
    is_leaf: bool,
    index: u8,
    pixel_count: u64,
    sum: (u64, u64, u64),
    avg: (f64, f64, f64),
    error_sum: (u64, u64, u64),
    next_reducible: Option<usize>,
    children: [Option<usize>; 8],
}

/// The octree, as an arena of [`Node`]s. Port of the real `Node`
/// class's own tree-shaped recursive methods, made iterative-over-
/// indices where the original recursed over raw pointers.
struct Octree {
    nodes: Vec<Node>,
    /// Per-level (0..=depth) linked lists of reducible (non-leaf,
    /// already-created) nodes, threaded via `Node::next_reducible` --
    /// port of upstream's own `reducible_nodes` array of raw
    /// linked-list heads.
    reducible_heads: Vec<Option<usize>>,
    leaf_count: usize,
    depth: usize,
}

impl Octree {
    fn new(depth: usize) -> Self {
        let mut nodes = Vec::with_capacity(MAX_LEAVES * 2);
        nodes.push(Node::default()); // root, index 0
        Octree { nodes, reducible_heads: vec![None; depth + 1], leaf_count: 0, depth }
    }

    fn create_child(&mut self, level: usize) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(Node::default());
        if level == self.depth {
            self.nodes[idx].is_leaf = true;
            self.leaf_count += 1;
        } else {
            self.nodes[idx].next_reducible = self.reducible_heads[level];
            self.reducible_heads[level] = Some(idx);
        }
        idx
    }

    fn update_average(&mut self, node: usize) {
        let n = &mut self.nodes[node];
        let count = n.pixel_count as f64;
        n.avg = (n.sum.0 as f64 / count, n.sum.1 as f64 / count, n.sum.2 as f64 / count);
    }

    /// Port of `Node::add_color`, iterative over the arena.
    fn add_color(&mut self, r: u8, g: u8, b: u8) {
        let mut node = 0usize;
        let mut level = 0usize;
        loop {
            if self.nodes[node].is_leaf {
                {
                    let n = &mut self.nodes[node];
                    n.pixel_count += 1;
                    n.sum.0 += r as u64;
                    n.sum.1 += g as u64;
                    n.sum.2 += b as u64;
                }
                self.update_average(node);
                let n = &mut self.nodes[node];
                let (ar, ag, ab) = n.avg;
                n.error_sum.0 += (r as f64 - ar).abs() as u64;
                n.error_sum.1 += (g as f64 - ag).abs() as u64;
                n.error_sum.2 += (b as f64 - ab).abs() as u64;
                return;
            }
            let idx = get_index(r, g, b, level);
            let child = self.nodes[node].children[idx];
            let child = match child {
                Some(c) => c,
                None => {
                    let c = self.create_child(level);
                    self.nodes[node].children[idx] = Some(c);
                    c
                }
            };
            node = child;
            level += 1;
        }
    }

    fn total_error(&self, node: usize) -> u64 {
        let mut ans = 0u64;
        for child in self.nodes[node].children.iter().flatten() {
            let c = &self.nodes[*child];
            ans += c.error_sum.0 + c.error_sum.1 + c.error_sum.2;
        }
        ans
    }

    fn find_best_reducible_node(&self, head: Option<usize>) -> usize {
        let mut err = u64::MAX;
        let mut ans = head.expect("reduce() only called when a reducible node exists at this level");
        let mut q = head;
        while let Some(idx) = q {
            let e = self.total_error(idx);
            if e < err {
                err = e;
                ans = idx;
            }
            q = self.nodes[idx].next_reducible;
        }
        ans
    }

    /// Port of `Node::merge`: collapses `node`'s children back into
    /// it, turning it into a leaf. Returns how many children were
    /// merged (upstream uses this to adjust `leaf_count`).
    fn merge(&mut self, node: usize) -> u32 {
        let mut num = 0u32;
        let children = self.nodes[node].children;
        for child in children.into_iter().flatten() {
            let (sr, sg, sb) = self.nodes[child].sum;
            let (er, eg, eb) = self.nodes[child].error_sum;
            let pc = self.nodes[child].pixel_count;
            let n = &mut self.nodes[node];
            n.sum.0 += sr;
            n.sum.1 += sg;
            n.sum.2 += sb;
            n.error_sum.0 += er;
            n.error_sum.1 += eg;
            n.error_sum.2 += eb;
            n.pixel_count += pc;
            num += 1;
            // Real upstream returns freed nodes to a pool for reuse;
            // this arena never reclaims slots (a real, disclosed
            // simplification -- see this module's own doc). The
            // merged child's own slot is simply abandoned (never
            // visited again, since its parent no longer references
            // it).
        }
        self.update_average(node);
        let n = &mut self.nodes[node];
        n.is_leaf = true;
        n.children = [None; 8];
        num
    }

    /// Port of `Node::reduce`, called on the tree root.
    fn reduce(&mut self) {
        let mut i = self.depth - 1;
        while i > 0 && self.reducible_heads[i].is_none() {
            i -= 1;
        }
        let head = self.reducible_heads[i];
        let node = self.find_best_reducible_node(head);

        // Unlink `node` from the level-`i` reducible list.
        if self.reducible_heads[i] == Some(node) {
            self.reducible_heads[i] = self.nodes[node].next_reducible;
        } else {
            let mut q = self.reducible_heads[i];
            while let Some(qi) = q {
                if self.nodes[qi].next_reducible == Some(node) {
                    self.nodes[qi].next_reducible = self.nodes[node].next_reducible;
                    break;
                }
                q = self.nodes[qi].next_reducible;
            }
        }
        let merged = self.merge(node);
        self.leaf_count -= (merged - 1) as usize;
    }

    fn reduce_to(&mut self, maximum_colors: usize) {
        while self.leaf_count > maximum_colors {
            self.reduce();
        }
    }

    /// Port of `Node::set_palette_colors`, run from the root.
    fn set_palette_colors(&mut self, node: usize, color_table: &mut Vec<[u8; 3]>) {
        if self.nodes[node].is_leaf {
            let (r, g, b) = self.nodes[node].avg;
            self.nodes[node].index = color_table.len() as u8;
            color_table.push([r.round() as u8, g.round() as u8, b.round() as u8]);
        } else {
            let children = self.nodes[node].children;
            for child in children.into_iter().flatten() {
                self.set_palette_colors(child, color_table);
                let (cpc, cavg) = (self.nodes[child].pixel_count, self.nodes[child].avg);
                let n = &mut self.nodes[node];
                n.pixel_count += cpc;
                n.sum.0 += (cpc as f64 * cavg.0) as u64;
                n.sum.1 += (cpc as f64 * cavg.1) as u64;
                n.sum.2 += (cpc as f64 * cavg.2) as u64;
            }
            self.update_average(node);
        }
    }

    /// Port of `Node::index_for_nearest_color`, run from the root.
    fn index_for_nearest_color(&self, r: u8, g: u8, b: u8) -> u8 {
        let mut node = 0usize;
        let mut level = 0usize;
        loop {
            if self.nodes[node].is_leaf {
                return self.nodes[node].index;
            }
            let mut idx = get_index(r, g, b, level);
            if self.nodes[node].children[idx].is_none() {
                let mut min_distance = i64::MAX;
                for (i, child) in self.nodes[node].children.iter().enumerate() {
                    if let Some(c) = child {
                        let (ar, ag, ab) = self.nodes[*c].avg;
                        let d = euclidean_distance(r as i64, g as i64, b as i64, ar as i64, ag as i64, ab as i64);
                        if d < min_distance {
                            min_distance = d;
                            idx = i;
                        }
                    }
                }
            }
            node = self.nodes[node].children[idx].expect("a nearest child was just found or already existed");
            level += 1;
        }
    }
}

fn read_colors(octree: &mut Octree, image: &RgbaImage) {
    for p in image.pixels() {
        octree.add_color(p[0], p[1], p[2]);
        while octree.leaf_count > MAX_LEAVES {
            octree.reduce();
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct DoublePixel {
    r: f64,
    g: f64,
    b: f64,
}

const ZERO_ERR: DoublePixel = DoublePixel { r: 0.0, g: 0.0, b: 0.0 };

fn propagate_error(line: &mut [DoublePixel], c: usize, mult: f64, error: DoublePixel) {
    line[c].r += error.r * mult;
    line[c].g += error.g * mult;
    line[c].b += error.b * mult;
}

fn apply_error(pixel: [u8; 3], error: DoublePixel) -> [u8; 3] {
    let clamp = |v: f64| v.clamp(0.0, 255.0) as u8;
    [clamp(pixel[0] as f64 + error.r), clamp(pixel[1] as f64 + error.g), clamp(pixel[2] as f64 + error.b)]
}

fn calculate_error(new_pixel: [u8; 3], old_pixel: [u8; 3]) -> DoublePixel {
    DoublePixel { r: (old_pixel[0] as f64 - new_pixel[0] as f64) / 16.0, g: (old_pixel[1] as f64 - new_pixel[1] as f64) / 16.0, b: (old_pixel[2] as f64 - new_pixel[2] as f64) / 16.0 }
}

/// Port of `dither_image`: real Floyd-Steinberg error diffusion with
/// serpentine (boustrophedon) scan direction -- odd rows scan right
/// to left, matching upstream exactly (this halves the directional
/// bias plain FS dithering otherwise has).
fn dither_image(image: &RgbaImage, octree: &Octree, color_table: &[[u8; 3]]) -> Vec<u8> {
    let (w, h) = image.dimensions();
    let (iw, ih) = (w as usize, h as usize);
    let mut indices = vec![0u8; iw * ih];
    let mut err1 = vec![ZERO_ERR; iw];
    let mut err2 = vec![ZERO_ERR; iw];

    for r in 0..ih {
        let is_odd = r & 1 == 1;
        err2.fill(ZERO_ERR);
        let (start, delta): (i64, i64) = if is_odd { (iw as i64 - 1, -1) } else { (0, 1) };
        let mut c = start;
        loop {
            let remaining = if is_odd { c + 1 } else { iw as i64 - c };
            if remaining <= 0 {
                break;
            }
            let cu = c as usize;
            let p = image.get_pixel(cu as u32, r as u32);
            let pixel = [p[0], p[1], p[2]];
            let err_pixel = apply_error(pixel, err1[cu]);
            let index = octree.index_for_nearest_color(err_pixel[0], err_pixel[1], err_pixel[2]);
            indices[r * iw + cu] = index;
            let error = calculate_error(color_table[index as usize], pixel);

            let has_forward = if is_odd { c > 0 } else { c < iw as i64 - 1 };
            if has_forward {
                propagate_error(&mut err1, (c + delta) as usize, 7.0, error);
                propagate_error(&mut err2, (c + delta) as usize, 1.0, error);
            }
            propagate_error(&mut err2, cu, 5.0, error);
            let has_backward = if is_odd { c < iw as i64 - 1 } else { c > 0 };
            if has_backward {
                propagate_error(&mut err2, (c - delta) as usize, 3.0, error);
            }

            c += delta;
        }
        std::mem::swap(&mut err1, &mut err2);
    }
    indices
}

fn write_image_nearest(image: &RgbaImage, octree: &Octree) -> Vec<u8> {
    let (w, h) = image.dimensions();
    let mut indices = vec![0u8; (w * h) as usize];
    for (i, p) in image.pixels().enumerate() {
        indices[i] = octree.index_for_nearest_color(p[0], p[1], p[2]);
    }
    indices
}

/// Port of `calibre.utils.img.quantize_image` (the real public entry
/// point; see this module's own doc for why this mirrors that wrapper
/// rather than the raw C++ `imageops.quantize`'s stricter alpha
/// precondition). `max_colors` is clamped to `[2, 256]`, matching
/// upstream. Returns an image the same size as `image`, with at most
/// `max_colors` distinct colors and alpha forced to fully opaque
/// (matching the real blend-away-transparency step).
pub fn quantize(image: &RgbaImage, max_colors: u32, dither: bool) -> RgbaImage {
    let max_colors = (max_colors.max(2) as usize).min(MAX_COLORS);

    let opaque;
    let source: &RgbaImage = if image.pixels().any(|p| p[3] != 255) {
        let (w, h) = image.dimensions();
        let mut canvas = RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255]));
        let _ = overlay(&mut canvas, image, 0, 0);
        opaque = canvas;
        &opaque
    } else {
        image
    };

    // Real, disclosed bug fix, not a reproduced quirk: upstream's own
    // `create_child`/`add_color` create a leaf when the PARENT's
    // level equals `depth`, meaning the parent itself must first call
    // `get_index(..., level=depth)` before that leaf gets created. At
    // `depth == MAX_DEPTH` (8) -- reachable for the common
    // `max_colors >= 256` case, real Python's own default -- this
    // calls `get_index` with `level == 8`, one past `BIT_MASK`'s real
    // 8-entry bound (indices 0..=7), an out-of-bounds C++ array read
    // (undefined behavior in the original, not something safe Rust
    // can or should reproduce byte-for-byte). Capping the effective
    // depth at `MAX_DEPTH - 1` keeps every `get_index` call within
    // its real valid range; the quality cost (7 vs. 8 bits of octree
    // branching) is negligible for real images.
    let depth = ((max_colors as f64).log2() as usize).clamp(2, MAX_DEPTH - 1);
    let mut octree = Octree::new(depth);
    read_colors(&mut octree, source);
    octree.reduce_to(max_colors);

    let mut color_table = Vec::with_capacity(octree.leaf_count);
    octree.set_palette_colors(0, &mut color_table);

    let indices = if dither { dither_image(source, &octree, &color_table) } else { write_image_nearest(source, &octree) };

    let (w, h) = source.dimensions();
    let mut out = RgbaImage::new(w, h);
    for (i, idx) in indices.iter().enumerate() {
        let [r, g, b] = color_table[*idx as usize];
        out.put_pixel((i as u32) % w, (i as u32) / w, Rgba([r, g, b, 255]));
    }
    out
}

// ===================================================================
// ordered_dither (issue #571's second real file, ordered_dither.cpp)
// ===================================================================

const THRESHOLD_MAP_O8X8: [u8; 64] = [1, 49, 13, 61, 4, 52, 16, 64, 33, 17, 45, 29, 36, 20, 48, 32, 9, 57, 5, 53, 12, 60, 8, 56, 41, 25, 37, 21, 44, 28, 40, 24, 3, 51, 15, 63, 2, 50, 14, 62, 35, 19, 47, 31, 34, 18, 46, 30, 11, 59, 7, 55, 10, 58, 6, 54, 43, 27, 39, 23, 42, 26, 38, 22];

fn div255(v: u32) -> u32 {
    let v = v + 128;
    ((v >> 8) + v) >> 8
}

/// Port of `dither_o8x8`: quantizes one grayscale value down to a
/// 16-level palette (evenly spaced 0,17,34,...,255 -- the real e-ink
/// palette) using an 8x8 ordered dither pattern, real ImageMagick
/// algorithm (`threshold_map_o8x8`), reproduced with the exact same
/// integer-only math (no floating point) upstream uses.
fn dither_o8x8(x: u32, y: u32, v: u8) -> u8 {
    let t = div255(v as u32 * ((15u32 << 6) + 1));
    let l = t >> 6;
    let t = t - (l << 6);
    let threshold = THRESHOLD_MAP_O8X8[((x & 7) + 8 * (y & 7)) as usize] as u32;
    let q = (l + u32::from(t >= threshold)) * 17;
    q.min(255) as u8
}

/// Port of `ordered_dither`: converts `image` to a real grayscale,
/// 16-level e-ink-palette-dithered image, one byte per pixel. Real
/// upstream returns a `Format_Grayscale8` `QImage`; this port returns
/// the raw grayscale byte buffer directly (row-major, matching that
/// format's own layout) since [`image::RgbaImage`] has no
/// single-channel counterpart in this crate's own established
/// image-type vocabulary elsewhere.
pub fn ordered_dither(image: &RgbaImage) -> (u32, u32, Vec<u8>) {
    let (w, h) = image.dimensions();
    let is_gray = image.pixels().all(|p| p[0] == p[1] && p[1] == p[2]);
    let mut out = vec![0u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let p = image.get_pixel(x, y);
            let gray = if is_gray { p[0] } else { crate::imageops::q_gray(p[0], p[1], p[2]) };
            out[(y * w + x) as usize] = dither_o8x8(x, y, gray);
        }
    }
    (w, h, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba(rgba))
    }

    // ===============================================================
    // quantize
    // ===============================================================

    #[test]
    fn quantize_a_two_color_image_reproduces_it_exactly() {
        let mut img = RgbaImage::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                let color = if x < 2 { [255, 0, 0, 255] } else { [0, 0, 255, 255] };
                img.put_pixel(x, y, Rgba(color));
            }
        }
        let out = quantize(&img, 256, false);
        assert_eq!(out, img, "a 2-color image well under the max_colors budget should round-trip exactly");
    }

    #[test]
    fn quantize_reduces_a_gradient_to_the_requested_color_count() {
        let mut img = RgbaImage::new(64, 1);
        for x in 0..64 {
            let v = (x * 4) as u8;
            img.put_pixel(x, 0, Rgba([v, v, v, 255]));
        }
        let out = quantize(&img, 4, false);
        let mut distinct = std::collections::HashSet::new();
        for p in out.pixels() {
            distinct.insert(p.0);
        }
        assert!(distinct.len() <= 4, "expected at most 4 distinct colors, got {}: {:?}", distinct.len(), distinct);
    }

    #[test]
    fn quantize_blends_transparency_onto_white_before_quantizing() {
        let img = solid(2, 2, [10, 20, 30, 0]); // fully transparent
        let out = quantize(&img, 256, false);
        // Blended onto white at alpha=0 should yield pure white.
        assert_eq!(out.get_pixel(0, 0).0, [255, 255, 255, 255]);
    }

    #[test]
    fn quantize_output_is_always_fully_opaque() {
        let img = solid(3, 3, [50, 60, 70, 128]);
        let out = quantize(&img, 256, false);
        for p in out.pixels() {
            assert_eq!(p[3], 255);
        }
    }

    #[test]
    fn quantize_clamps_max_colors_to_the_valid_range() {
        let img = solid(4, 4, [1, 2, 3, 255]);
        // Should not panic for out-of-range requests.
        let _ = quantize(&img, 0, false);
        let _ = quantize(&img, 100_000, false);
    }

    #[test]
    fn quantize_with_dithering_stays_within_the_requested_palette_size() {
        let mut img = RgbaImage::new(32, 32);
        for y in 0..32 {
            for x in 0..32 {
                let v = ((x + y) * 4) as u8;
                img.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }
        let out = quantize(&img, 8, true);
        let mut distinct = std::collections::HashSet::new();
        for p in out.pixels() {
            distinct.insert(p.0);
        }
        assert!(distinct.len() <= 8, "dithering must only ever select among the real palette colors: {} found", distinct.len());
    }

    #[test]
    fn quantize_dithering_changes_output_relative_to_nearest_color_mapping() {
        let mut img = RgbaImage::new(16, 16);
        for y in 0..16 {
            for x in 0..16 {
                let v = ((x as f32 / 16.0) * 255.0) as u8;
                img.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }
        let plain = quantize(&img, 3, false);
        let dithered = quantize(&img, 3, true);
        assert_ne!(plain, dithered, "dithering should visibly differ from plain nearest-color mapping on a gradient");
    }

    // ===============================================================
    // ordered_dither
    // ===============================================================

    #[test]
    fn ordered_dither_maps_black_and_white_to_the_extremes() {
        let (w, h, out) = ordered_dither(&solid(4, 4, [0, 0, 0, 255]));
        assert_eq!((w, h), (4, 4));
        assert!(out.iter().all(|&v| v == 0), "{out:?}");

        let (_, _, out) = ordered_dither(&solid(4, 4, [255, 255, 255, 255]));
        assert!(out.iter().all(|&v| v == 255), "{out:?}");
    }

    #[test]
    fn ordered_dither_quantizes_mid_gray_to_one_of_the_16_real_eink_levels() {
        let (_, _, out) = ordered_dither(&solid(8, 8, [128, 128, 128, 255]));
        for &v in &out {
            assert_eq!(v % 17, 0, "every output value must be a multiple of 17 (one of the 16 real e-ink levels): {v}");
        }
    }

    #[test]
    fn ordered_dither_uses_real_qgray_weights_for_non_gray_input() {
        // A pure, fully-saturated color is not gray -- confirm the
        // real qGray-weighted conversion path is used (not treated as
        // already-gray), by checking the result differs from what a
        // naive single-channel read would give.
        let (_, _, out) = ordered_dither(&solid(8, 8, [255, 0, 0, 255]));
        // qGray(255,0,0) = (255*11)/32 = 87 (rounded down), which
        // dithers to one of the two nearest 16-level buckets (85 or
        // 102), never to a red-channel-only reading of 255.
        assert!(out.iter().all(|&v| v == 85 || v == 102), "{out:?}");
    }

    #[test]
    fn ordered_dither_pattern_is_spatially_varying() {
        // The same input gray value at different (x,y) positions can
        // dither differently depending on the 8x8 threshold map --
        // confirm the function is not just a flat per-value LUT.
        let img = solid(8, 8, [100, 100, 100, 255]);
        let (_, _, out) = ordered_dither(&img);
        let distinct: std::collections::HashSet<u8> = out.iter().copied().collect();
        assert!(distinct.len() > 1, "a uniform mid-gray image should still show real spatial dither variation: {distinct:?}");
    }
}
