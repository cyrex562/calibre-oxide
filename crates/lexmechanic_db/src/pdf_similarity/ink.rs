//! The ink map a page hash is made from.

use super::widen;

/// A pixel is ink when it is this fraction of the paper level darker than its
/// neighbourhood's mean.
pub const INK_CONTRAST: f32 = 0.08;
/// A row or column of the render with an unbroken run of ink longer than this
/// fraction of it is a border.
pub const BORDER_INK_FRACTION: f32 = 0.55;
/// Gaps of up to this many pixels do not break a border's run (scan noise).
const BORDER_GAP: usize = 2;
/// A page with less ink than this fraction of its pixels is blank.
pub const MIN_INK_FRACTION: f32 = 0.02;
/// The ink's own percentile bounds the crop.
pub const CROP_PERCENTILE: f32 = 0.01;

/// Binarises a page against its own local background and returns the ink
/// density of the text block on a `gx` x `gy` grid (row-major), or `None` for
/// a blank page.
///
/// Why this and not a DCT of the gray page: the pages of one book are the same
/// gray texture (one font, one line grid), and a low-frequency transform only
/// sees the shape of the text block. What tells two pages of a book apart is
/// the *content* of each cell (a line that ends early, a heading, which
/// letters fall where), which survives a re-encode but not a DCT median.
/// Binarising first makes it insensitive to paper tone, lighting and the
/// strength of JPEG blur; the crop is taken from the ink's own 1st-99th
/// percentile, after rows and columns that are mostly ink (a scanner's border)
/// have been dropped, so it does not flip between two encodings of one image.
pub fn ink_grid(gray: &[u8], width: u32, height: u32, gx: usize, gy: usize) -> Option<Vec<f32>> {
    page_grids(gray, width, height, gx, gy).map(|g| g.ink)
}

/// What [`page_grids`] reads off the text block.
pub struct PageGrids {
    /// Share of ink pixels in each cell.
    pub ink: Vec<f32>,
    /// Mean gray level of each cell.
    pub mean: Vec<f32>,
}

/// [`ink_grid`] and the mean gray of the same cells.
pub fn page_grids(gray: &[u8], width: u32, height: u32, gx: usize, gy: usize) -> Option<PageGrids> {
    let (w, h) = (width as usize, height as usize);
    if w < 64 || h < 64 || gray.len() < w * h {
        return None;
    }
    // Integral image for the local mean.
    let iw = w + 1;
    let mut integral = vec![0u32; iw * (h + 1)];
    for y in 0..h {
        let mut row = 0u32;
        for x in 0..w {
            row += gray[y * w + x] as u32;
            integral[(y + 1) * iw + x + 1] = integral[y * iw + x + 1] + row;
        }
    }
    let radius = (w.max(h) / 80).max(4);
    let mean_at = |x: usize, y: usize| -> f32 {
        let (x0, x1) = (x.saturating_sub(radius), (x + radius + 1).min(w));
        let (y0, y1) = (y.saturating_sub(radius), (y + radius + 1).min(h));
        let sum = integral[y1 * iw + x1] + integral[y0 * iw + x0] - integral[y0 * iw + x1] - integral[y1 * iw + x0];
        sum as f32 / ((x1 - x0) * (y1 - y0)) as f32
    };
    // Paper level: the 90th percentile of a sample.
    let mut sample: Vec<u8> = gray.iter().step_by(7).copied().collect();
    sample.sort_unstable();
    let paper = sample[sample.len() * 9 / 10] as f32;
    let threshold = (paper * INK_CONTRAST).max(10.0);

    let mut ink = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            if (gray[y * w + x] as f32) + threshold < mean_at(x, y) {
                ink[y * w + x] = true;
            }
        }
    }
    // A column or row with one unbroken run of ink across most of it is a
    // scanner's border, not text: a line of text, however dense, breaks at
    // every word gap. Gaps of up to `BORDER_GAP` pixels (noise) do not break
    // a run.
    let mut row_run = vec![0usize; h];
    let mut col_run = vec![0usize; w];
    {
        // (current run length, pixels since the last ink) per row and column.
        let mut col_state = vec![(0usize, usize::MAX); w];
        for y in 0..h {
            let mut state = (0usize, usize::MAX);
            for x in 0..w {
                let on = ink[y * w + x];
                for (s, best) in [(&mut state, &mut row_run[y]), (&mut col_state[x], &mut col_run[x])] {
                    if on {
                        s.0 = if s.1 <= BORDER_GAP { s.0 + s.1 + 1 } else { 1 };
                        s.1 = 0;
                        *best = (*best).max(s.0);
                    } else {
                        s.1 = s.1.saturating_add(1);
                    }
                }
            }
        }
    }
    let mut drop_col = vec![false; w];
    let mut drop_row = vec![false; h];
    for x in 0..w {
        if col_run[x] as f32 > BORDER_INK_FRACTION * h as f32 {
            for d in drop_col.iter_mut().take((x + 3).min(w)).skip(x.saturating_sub(2)) {
                *d = true;
            }
        }
    }
    for y in 0..h {
        if row_run[y] as f32 > BORDER_INK_FRACTION * w as f32 {
            for d in drop_row.iter_mut().take((y + 3).min(h)).skip(y.saturating_sub(2)) {
                *d = true;
            }
        }
    }
    let mut colc = vec![0u32; w];
    let mut rowc = vec![0u32; h];
    let mut total = 0u64;
    for y in 0..h {
        for x in 0..w {
            if ink[y * w + x] && !drop_col[x] && !drop_row[y] {
                colc[x] += 1;
                rowc[y] += 1;
                total += 1;
            } else {
                ink[y * w + x] = false;
            }
        }
    }
    if (total as f32) < MIN_INK_FRACTION * (w * h) as f32 {
        return None;
    }
    let span = |hist: &[u32]| -> (usize, usize) {
        let cut = (total as f32 * CROP_PERCENTILE) as u64;
        let (mut cum, mut lo) = (0u64, 0usize);
        for (i, c) in hist.iter().enumerate() {
            cum += *c as u64;
            if cum > cut {
                lo = i;
                break;
            }
        }
        cum = 0;
        let mut hi = hist.len() - 1;
        for (i, c) in hist.iter().enumerate().rev() {
            cum += *c as u64;
            if cum > cut {
                hi = i;
                break;
            }
        }
        (lo, hi.max(lo) + 1)
    };
    let (cl, ch) = span(&colc);
    let (rl, rh) = span(&rowc);
    let (x0, x1) = widen(cl, ch, w);
    let (y0, y1) = widen(rl, rh, h);

    // Integral of the ink, to read cell sums.
    let mut ii = vec![0u32; iw * (h + 1)];
    for y in 0..h {
        let mut row = 0u32;
        for x in 0..w {
            row += ink[y * w + x] as u32;
            ii[(y + 1) * iw + x + 1] = ii[y * iw + x + 1] + row;
        }
    }
    let mut out = Vec::with_capacity(gx * gy);
    let mut means = Vec::with_capacity(gx * gy);
    for cy in 0..gy {
        let ya = y0 + cy * (y1 - y0) / gy;
        let yb = (y0 + (cy + 1) * (y1 - y0) / gy).max(ya + 1);
        for cx in 0..gx {
            let xa = x0 + cx * (x1 - x0) / gx;
            let xb = (x0 + (cx + 1) * (x1 - x0) / gx).max(xa + 1);
            let sum = ii[yb * iw + xb] + ii[ya * iw + xa] - ii[ya * iw + xb] - ii[yb * iw + xa];
            out.push(sum as f32 / ((xb - xa) * (yb - ya)) as f32);
            let gsum = integral[yb * iw + xb] + integral[ya * iw + xa] - integral[ya * iw + xb] - integral[yb * iw + xa];
            means.push(gsum as f32 / ((xb - xa) * (yb - ya)) as f32);
        }
    }
    Some(PageGrids { ink: out, mean: means })
}
