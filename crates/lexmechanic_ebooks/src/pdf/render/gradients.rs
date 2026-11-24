//! Port of `old_src/src/calibre/ebooks/pdf/render/gradients.py` (150 lines).
//!
//! `LinearGradientPattern`'s PDF shading-dictionary construction and
//! `spread_gradient`'s pad/reflect/repeat spread-expansion math are pure
//! logic; only the *inputs* were Qt types (`QLinearGradient`,
//! `QTransform`, `QPointF`). Ported for real, no gaps, against plain-data
//! stand-ins:
//!
//! [`Matrix`] is a 2D affine transform (`m11 m12 m21 m22 dx dy`, Qt's
//! `QTransform` row-vector convention: a point `(x, y)` maps to `(x*m11 +
//! y*m21 + dx, x*m12 + y*m22 + dy)`), with the `invert`/`map_point`
//! operations `spread_gradient` needs. No matrix type existed elsewhere in
//! this crate at the time of this port.
//!
//! [`Gradient`] replaces `QLinearGradient`: a start/stop point pair, `(t,
//! rgba)` color stops, and a [`SpreadKind`] (mirrors Qt's
//! `QGradient::Spread` enum: `PadSpread`/`ReflectSpread`/`RepeatSpread`).

// ==========================================================================
// Matrix
// ==========================================================================

/// A 2D affine transform, the plain-data stand-in for Qt's `QTransform`
/// as used by this module (only the subset `spread_gradient` needs:
/// `inverted()` and `map()`/composition).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix {
    pub m11: f64,
    pub m12: f64,
    pub m21: f64,
    pub m22: f64,
    pub dx: f64,
    pub dy: f64,
}

impl Matrix {
    pub fn identity() -> Self {
        Matrix {
            m11: 1.0,
            m12: 0.0,
            m21: 0.0,
            m22: 1.0,
            dx: 0.0,
            dy: 0.0,
        }
    }

    pub fn translate(dx: f64, dy: f64) -> Self {
        Matrix {
            dx,
            dy,
            ..Matrix::identity()
        }
    }

    /// Maps a point through this transform: `Qt`'s `QTransform::map`.
    pub fn map_point(&self, x: f64, y: f64) -> (f64, f64) {
        (
            x * self.m11 + y * self.m21 + self.dx,
            x * self.m12 + y * self.m22 + self.dy,
        )
    }

    /// `self * other`: apply `self` first, then `other` - matches Qt's
    /// `QTransform::operator*` composition order (`t1 * t2` means "apply
    /// t1, then t2").
    pub fn then(&self, other: &Matrix) -> Matrix {
        Matrix {
            m11: self.m11 * other.m11 + self.m12 * other.m21,
            m12: self.m11 * other.m12 + self.m12 * other.m22,
            m21: self.m21 * other.m11 + self.m22 * other.m21,
            m22: self.m21 * other.m12 + self.m22 * other.m22,
            dx: self.dx * other.m11 + self.dy * other.m21 + other.dx,
            dy: self.dx * other.m12 + self.dy * other.m22 + other.dy,
        }
    }

    /// `QTransform::inverted()`. Returns `None` for a singular
    /// (non-invertible) matrix; Qt itself returns an identity matrix in
    /// that case with an `invertible` out-flag set false - callers here
    /// that mirror Qt's `.inverted()[0]` should fall back to
    /// [`Matrix::identity`] on `None`, matching that.
    pub fn invert(&self) -> Option<Matrix> {
        let det = self.m11 * self.m22 - self.m12 * self.m21;
        if det == 0.0 {
            return None;
        }
        let m11 = self.m22 / det;
        let m12 = -self.m12 / det;
        let m21 = -self.m21 / det;
        let m22 = self.m11 / det;
        let dx = -(self.dx * m11 + self.dy * m21);
        let dy = -(self.dx * m12 + self.dy * m22);
        Some(Matrix {
            m11,
            m12,
            m21,
            m22,
            dx,
            dy,
        })
    }

    pub fn as_tuple(&self) -> [f64; 6] {
        [self.m11, self.m12, self.m21, self.m22, self.dx, self.dy]
    }
}

fn point_sub(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 - b.0, a.1 - b.1)
}
fn point_add(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 + b.0, a.1 + b.1)
}

// ==========================================================================
// Gradient / spread_gradient
// ==========================================================================

/// Port of Qt's `QGradient::Spread` enum as used by this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpreadKind {
    Pad,
    Reflect,
    Repeat,
}

/// A single gradient color stop: `(position 0..=1, rgba 0..=1)`. Port of
/// `gradient.stops()`'s `(t, QColor)` pairs, `QColor` reduced to its
/// `getRgbF()` 4-tuple.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stop {
    pub t: f64,
    pub color: [f64; 4],
}

/// Plain-data stand-in for `QLinearGradient` (plus the brush's spread
/// mode) - everything `spread_gradient`/`LinearGradientPattern` read off
/// the Qt gradient object.
#[derive(Debug, Clone)]
pub struct Gradient {
    pub start: (f64, f64),
    pub stop: (f64, f64),
    pub stops: Vec<Stop>,
    pub spread: SpreadKind,
}

fn in_page(point: (f64, f64), minx: f64, maxx: f64, miny: f64, maxy: f64) -> bool {
    minx <= point.0 && point.0 <= maxx && miny <= point.1 && point.1 <= maxy
}

/// Port of `LinearGradientPattern.spread_gradient` (gradients.py lines
/// 78-150): for non-`Pad` spreads, extends the stop list by repeating
/// (optionally mirroring) the base stop sequence outward until it covers
/// the full page (mapped through the inverse of `matrix`), then
/// re-derives each replicated stop's `t` position along the new,
/// extended start/stop span.
///
/// Returns the (possibly extended) `(start, stop, stops)`.
pub fn spread_gradient(
    gradient: &Gradient,
    pixel_page_width: f64,
    pixel_page_height: f64,
    matrix: &Matrix,
) -> ((f64, f64), (f64, f64), Vec<Stop>) {
    let mut start = gradient.start;
    let mut stop = gradient.stop;
    let mut stops = gradient.stops.clone();

    if gradient.spread == SpreadKind::Pad {
        return (start, stop, stops);
    }

    let inv = matrix.invert().unwrap_or_else(Matrix::identity);
    let page_rect = [
        inv.map_point(0.0, 0.0),
        inv.map_point(pixel_page_width, 0.0),
        inv.map_point(0.0, pixel_page_height),
        inv.map_point(pixel_page_width, pixel_page_height),
    ];
    let mut minx = f64::INFINITY;
    let mut maxx = f64::NEG_INFINITY;
    let mut miny = f64::INFINITY;
    let mut maxy = f64::NEG_INFINITY;
    for &(x, y) in &page_rect {
        minx = minx.min(x);
        maxx = maxx.max(x);
        miny = miny.min(y);
        maxy = maxy.max(y);
    }

    let offset = point_sub(stop, start);
    let mut llimit = start;
    let mut rlimit = stop;

    let base_stops = stops.clone();
    let mut reversed_stops = base_stops.clone();
    reversed_stops.reverse();
    let do_reflect = gradient.spread == SpreadKind::Reflect;
    let totl = (base_stops.last().unwrap().t - base_stops[0].t).abs();
    let intervals: Vec<f64> = (0..base_stops.len().saturating_sub(1))
        .map(|i| (base_stops[i + 1].t - base_stops[i].t).abs() / totl)
        .collect();

    let mut reflect = false;
    while in_page(llimit, minx, maxx, miny, maxy) {
        reflect = !reflect;
        llimit = point_sub(llimit, offset);
        let estops = if reflect && do_reflect {
            &reversed_stops
        } else {
            &base_stops
        };
        let mut new_stops = estops.clone();
        new_stops.extend(stops);
        stops = new_stops;
    }
    let first_is_reflected = reflect;
    reflect = false;

    while in_page(rlimit, minx, maxx, miny, maxy) {
        reflect = !reflect;
        rlimit = point_add(rlimit, offset);
        let estops = if reflect && do_reflect {
            &reversed_stops
        } else {
            &base_stops
        };
        stops.extend(estops.clone());
    }

    start = llimit;
    stop = rlimit;

    let num = stops.len() / base_stops.len();
    if num > 1 {
        let mut t = base_stops[0].t;
        let rlen = totl / num as f64;
        let mut reflect = !first_is_reflected;
        let intervals: Vec<f64> = intervals.iter().map(|i| i * rlen).collect();
        let rintervals: Vec<f64> = intervals.iter().rev().copied().collect();

        for i in 0..num {
            reflect = !reflect;
            let pos = i * base_stops.len();
            let mut tvals = vec![t];
            let use_intervals = if reflect && do_reflect {
                &rintervals
            } else {
                &intervals
            };
            for &ival in use_intervals {
                tvals.push(tvals.last().unwrap() + ival);
            }
            for j in 0..base_stops.len() {
                stops[pos + j].t = tvals[j];
            }
            t = *tvals.last().unwrap();
        }
        let last = stops.len() - 1;
        stops[last].t = base_stops.last().unwrap().t;
    }

    (start, stop, stops)
}

// ==========================================================================
// LinearGradientPattern
// ==========================================================================

use super::common::{Array, Dictionary, Name, PdfObj};

/// Port of `LinearGradientPattern` (gradients.py lines 19-76): a PDF
/// axial-shading pattern dictionary built from a (possibly
/// spread-expanded) linear gradient.
#[derive(Debug, Clone)]
pub struct LinearGradientPattern {
    pub dict: Dictionary,
    pub matrix: Matrix,
    pub const_opacity: f64,
    /// Dedup key: mirrors Python's `(class name, matrix, coords, stops)`
    /// tuple, flattened to a comparable/hashable string.
    pub cache_key: String,
}

fn fmt_f64_key(f: f64) -> String {
    format!("{:.12e}", f)
}

impl LinearGradientPattern {
    /// Port of `LinearGradientPattern.__init__` (gradients.py lines
    /// 21-76). `gradient` and `matrix` replace the Qt `brush`/`QTransform`
    /// parameters (see module doc comment); `pdf` (used only to build a
    /// cache key against calibre's live `PDFStream` in the original) is
    /// dropped since [`LinearGradientPattern::cache_key`] is
    /// self-contained here.
    pub fn new(
        gradient: &Gradient,
        matrix: &Matrix,
        pixel_page_width: f64,
        pixel_page_height: f64,
    ) -> LinearGradientPattern {
        let (start, stop, stops) =
            spread_gradient(gradient, pixel_page_width, pixel_page_height, matrix);
        let const_opacity = stops[0].color[3];

        let mut funcs = Array::new();
        let mut bounds = Array::new();
        let mut encode = Array::new();

        for i in 0..stops.len() {
            if i < stops.len() - 1 {
                let current = &stops[i];
                let next = &stops[i + 1];
                let mut func = Dictionary::new();
                func.insert("FunctionType", 2i64);
                func.insert("Domain", {
                    let mut a = Array::new();
                    a.push(0i64);
                    a.push(1i64);
                    a
                });
                func.insert("C0", {
                    let mut a = Array::new();
                    a.extend(current.color[..3].iter().copied());
                    a
                });
                func.insert("C1", {
                    let mut a = Array::new();
                    a.extend(next.color[..3].iter().copied());
                    a
                });
                func.insert("N", 1i64);
                funcs.push(func);
                encode.push(0i64);
                encode.push(1i64);
                if i + 1 < stops.len() - 1 {
                    bounds.push(next.t);
                }
            }
        }

        let mut func = Dictionary::new();
        func.insert("FunctionType", 3i64);
        func.insert("Domain", {
            let mut a = Array::new();
            a.push(stops[0].t);
            a.push(stops[stops.len() - 1].t);
            a
        });
        func.insert("Functions", funcs);
        func.insert("Bounds", bounds);
        func.insert("Encode", encode);

        let mut shader = Dictionary::new();
        shader.insert("ShadingType", 2i64);
        shader.insert("ColorSpace", Name::new("DeviceRGB"));
        shader.insert("AntiAlias", true);
        let coords = {
            let mut a = Array::new();
            a.push(start.0);
            a.push(start.1);
            a.push(stop.0);
            a.push(stop.1);
            a
        };
        shader.insert("Coords", coords.clone());
        shader.insert("Function", func);
        shader.insert("Extend", {
            let mut a = Array::new();
            a.push(true);
            a.push(true);
            a
        });

        let mut dict = Dictionary::new();
        dict.insert("Type", Name::new("Pattern"));
        dict.insert("PatternType", 2i64);
        dict.insert("Shading", shader);
        let mat = matrix.as_tuple();
        dict.insert("Matrix", {
            let mut a = Array::new();
            a.extend(mat.iter().copied());
            a
        });

        let mut key_parts = vec!["LinearGradientPattern".to_string()];
        key_parts.extend(mat.iter().map(|v| fmt_f64_key(*v)));
        for o in &coords.0 {
            if let PdfObj::Real(v) = o {
                key_parts.push(fmt_f64_key(*v));
            }
        }
        for s in &stops {
            key_parts.push(fmt_f64_key(s.t));
            for c in s.color {
                key_parts.push(fmt_f64_key(c));
            }
        }
        let cache_key = key_parts.join("|");

        LinearGradientPattern {
            dict,
            matrix: *matrix,
            const_opacity,
            cache_key,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(t: f64, r: f64, g: f64, b: f64, a: f64) -> Stop {
        Stop {
            t,
            color: [r, g, b, a],
        }
    }

    // ---- Matrix -------------------------------------------------------

    #[test]
    fn matrix_identity_maps_point_unchanged() {
        let m = Matrix::identity();
        assert_eq!(m.map_point(3.0, 4.0), (3.0, 4.0));
    }

    #[test]
    fn matrix_translate_shifts_point() {
        let m = Matrix::translate(10.0, -5.0);
        assert_eq!(m.map_point(1.0, 1.0), (11.0, -4.0));
    }

    #[test]
    fn matrix_invert_round_trips() {
        let m = Matrix {
            m11: 2.0,
            m12: 0.0,
            m21: 0.0,
            m22: 3.0,
            dx: 5.0,
            dy: -2.0,
        };
        let inv = m.invert().unwrap();
        let p = m.map_point(7.0, 8.0);
        let back = inv.map_point(p.0, p.1);
        assert!((back.0 - 7.0).abs() < 1e-9);
        assert!((back.1 - 8.0).abs() < 1e-9);
    }

    #[test]
    fn matrix_singular_has_no_inverse() {
        let m = Matrix {
            m11: 0.0,
            m12: 0.0,
            m21: 0.0,
            m22: 0.0,
            dx: 0.0,
            dy: 0.0,
        };
        assert!(m.invert().is_none());
    }

    // ---- spread_gradient: pad (trivial) --------------------------------

    #[test]
    fn spread_gradient_pad_is_a_no_op() {
        let g = Gradient {
            start: (0.0, 0.0),
            stop: (100.0, 0.0),
            stops: vec![stop(0.0, 1.0, 0.0, 0.0, 1.0), stop(1.0, 0.0, 0.0, 1.0, 1.0)],
            spread: SpreadKind::Pad,
        };
        let (s, e, stops) = spread_gradient(&g, 200.0, 200.0, &Matrix::identity());
        assert_eq!(s, (0.0, 0.0));
        assert_eq!(e, (100.0, 0.0));
        assert_eq!(stops.len(), 2);
    }

    // ---- spread_gradient: reflect/repeat, worked by hand ----------------
    //
    // Gradient from x=15 to x=25 (identity matrix, so page-space ==
    // gradient space), 2 stops (t=0 -> red, t=1 -> blue), spread =
    // Reflect. Page is x in [0, 50] (pixel_page_width=50, identity
    // matrix maps the page corners directly to (0,0)/(50,0)/(0,50)/(50,50)).
    //
    // offset = stop - start = (10, 0). Trace of the Python algorithm:
    //
    // Left walk (llimit starts at start=15, walks by -offset each step,
    // prepending a stop block each time llimit is still inside [0,50]
    // *before* stepping):
    //   llimit=15 in [0,50] -> reflect=T, llimit=5;  prepend reversed=[blue,red]
    //   llimit=5  in [0,50] -> reflect=F, llimit=-5; prepend base=[red,blue]
    //   llimit=-5 NOT in [0,50] -> stop.
    // stops (color order) = [red,blue] + [blue,red] + [red,blue] = 6 stops
    // (right-to-left prepend order: base, then reversed, then original).
    // first_is_reflected = reflect at loop exit = False.
    //
    // Right walk (rlimit starts at stop=25, walks by +offset, appending):
    //   rlimit=25 in [0,50] -> reflect=T, rlimit=35; append reversed=[blue,red]
    //   rlimit=35 in [0,50] -> reflect=F, rlimit=45; append base=[red,blue]
    //   rlimit=45 in [0,50] -> reflect=T, rlimit=55; append reversed=[blue,red]
    //   rlimit=55 NOT in [0,50] -> stop.
    // Final color sequence (12 stops):
    //   red,blue,blue,red,red,blue, blue,red,red,blue,blue,red
    // start=-5, stop=55.
    #[test]
    fn spread_gradient_reflect_expands_stops_past_page_edges() {
        let g = Gradient {
            start: (15.0, 0.0),
            stop: (25.0, 0.0),
            stops: vec![stop(0.0, 1.0, 0.0, 0.0, 1.0), stop(1.0, 0.0, 0.0, 1.0, 1.0)],
            spread: SpreadKind::Reflect,
        };
        let (s, e, stops) = spread_gradient(&g, 50.0, 50.0, &Matrix::identity());

        assert_eq!(s, (-5.0, 0.0));
        assert_eq!(e, (55.0, 0.0));
        assert_eq!(stops.len(), 12);
        // t values must be non-decreasing across the reconstructed stop list.
        for w in stops.windows(2) {
            assert!(
                w[0].t <= w[1].t + 1e-9,
                "t values must be sorted: {:?}",
                stops
            );
        }
        // First and last stops are t=0/t=1 of the base range.
        assert!((stops[0].t - 0.0).abs() < 1e-9);
        assert!((stops[stops.len() - 1].t - 1.0).abs() < 1e-9);
        let red = [1.0, 0.0, 0.0, 1.0];
        let blue = [0.0, 0.0, 1.0, 1.0];
        let expected_colors = [
            red, blue, blue, red, red, blue, blue, red, red, blue, blue, red,
        ];
        let actual_colors: Vec<[f64; 4]> = stops.iter().map(|s| s.color).collect();
        assert_eq!(actual_colors, expected_colors);
    }

    #[test]
    fn spread_gradient_repeat_does_not_mirror_colors() {
        let g = Gradient {
            start: (15.0, 0.0),
            stop: (25.0, 0.0),
            stops: vec![stop(0.0, 1.0, 0.0, 0.0, 1.0), stop(1.0, 0.0, 0.0, 1.0, 1.0)],
            spread: SpreadKind::Repeat,
        };
        let (_s, _e, stops) = spread_gradient(&g, 50.0, 50.0, &Matrix::identity());
        // Every replicated block must start with red and end with blue -
        // Repeat never reverses, unlike Reflect.
        for block in stops.chunks(2) {
            assert_eq!(block[0].color, [1.0, 0.0, 0.0, 1.0]);
            assert_eq!(block[1].color, [0.0, 0.0, 1.0, 1.0]);
        }
    }

    // ---- LinearGradientPattern -------------------------------------------

    #[test]
    fn linear_gradient_pattern_builds_shading_dict() {
        let g = Gradient {
            start: (0.0, 0.0),
            stop: (10.0, 0.0),
            stops: vec![stop(0.0, 1.0, 0.0, 0.0, 0.5), stop(1.0, 0.0, 0.0, 1.0, 0.5)],
            spread: SpreadKind::Pad,
        };
        let pat = LinearGradientPattern::new(&g, &Matrix::identity(), 100.0, 100.0);
        assert_eq!(pat.const_opacity, 0.5);
        assert_eq!(
            pat.dict.get("Type"),
            Some(&PdfObj::Name(Name::new("Pattern")))
        );
        assert_eq!(pat.dict.get("PatternType"), Some(&PdfObj::Int(2)));
        assert!(pat.dict.contains_key("Shading"));
        assert!(!pat.cache_key.is_empty());
    }

    #[test]
    fn linear_gradient_pattern_cache_key_is_stable_for_identical_inputs() {
        let g = Gradient {
            start: (0.0, 0.0),
            stop: (10.0, 0.0),
            stops: vec![stop(0.0, 1.0, 0.0, 0.0, 1.0), stop(1.0, 0.0, 0.0, 1.0, 1.0)],
            spread: SpreadKind::Pad,
        };
        let p1 = LinearGradientPattern::new(&g, &Matrix::identity(), 100.0, 100.0);
        let p2 = LinearGradientPattern::new(&g, &Matrix::identity(), 100.0, 100.0);
        assert_eq!(p1.cache_key, p2.cache_key);
    }
}
