//! Port of `old_src/src/calibre/ebooks/pdf/render/graphics.py` (484 lines).
//!
//! # Real: `convert_path`, tiling/hatch patterns, texture patterns
//!
//! [`convert_path`] turns a path-element list into `super::serialize::Path`.
//! The Python original scans a live `QPainterPath`'s element stream, where
//! a single cubic Bezier curve is represented as *three* consecutive
//! elements (`CurveToElement` + two `CurveToDataElement`s, an artifact of
//! Qt's internal path representation). Per the porting brief, the
//! plain-data stand-in for the input ([`PathElement`]) is defined the
//! natural way instead, with one `CurveTo` variant already carrying all
//! three points, so this port doesn't need to replicate Qt's
//! element-stream-scanning loop (including its "invalid curve to
//! operation" error path, which only exists to detect a malformed
//! *stream*, not a malformed curve; [`PathElement::CurveTo`] can't be
//! malformed in that sense).
//!
//! [`TilingPattern`]/[`qt_pattern`] (the hardcoded hatch-pattern content
//! streams and pattern-dictionary shape) and [`TexturePattern`] (adapted
//! per the porting brief to take `(image_ref, cache_key, width, height,
//! is_mono)` instead of extracting them from a live `QPixmap`) are ported
//! for real. `QtPattern` in Python is a trivial `TilingPattern` subclass
//! (constructor + one `write()` call); ported here as the free function
//! [`qt_pattern`] building a `TilingPattern` directly rather than adding
//! a near-empty wrapper type.
//!
//! # Blocked: `Graphics`/`GraphicsState`
//!
//! `GraphicsState` and `Graphics` (`update_state`, `__call__`,
//! `convert_brush`, `apply_stroke`, `apply_fill`, `resolve_fill`,
//! `begin`, `reset`) exist specifically to receive live calls from Qt's
//! `QPaintEngine`/`QPainter` during on-screen HTML rendering - there's no
//! non-Qt input in this codebase to drive them with, and building one
//! would mean writing an entire alternate HTML rendering pipeline, far
//! beyond a "port" of this module. This mirrors
//! `crate::pdf::image_writer`'s `draw_image_page`/`convert` gap - see
//! that file's module doc comment for the tone/wording this follows.
//!
//! [`GraphicsState`] itself is ported as a plain data struct (`Clone`/
//! `PartialEq` cover Python's trivial `.copy()`/`__eq__`, which never
//! touch Qt beyond holding its types) - only [`Graphics`]'s methods,
//! which actually process live paint-engine state, are `todo!()`'d.

use super::common::{Array, Dictionary, Name, PdfObj, Reference, Stream, StreamLike};
use super::gradients::Matrix;
use super::serialize::Path;

// ==========================================================================
// convert_path (graphics.py lines 18-40)
// ==========================================================================

/// Plain-data stand-in for a `QPainterPath` element list - see module
/// doc comment for why a single `CurveTo` variant already carries all
/// three Bezier points instead of mirroring Qt's 3-element curve
/// encoding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathElement {
    MoveTo(f64, f64),
    LineTo(f64, f64),
    CurveTo {
        c1: (f64, f64),
        c2: (f64, f64),
        end: (f64, f64),
    },
}

/// Port of `convert_path` (graphics.py lines 18-40).
pub fn convert_path(elements: &[PathElement]) -> Path {
    let mut p = Path::new();
    for el in elements {
        match *el {
            PathElement::MoveTo(x, y) => p.move_to(x, y),
            PathElement::LineTo(x, y) => p.line_to(x, y),
            PathElement::CurveTo { c1, c2, end } => {
                p.curve_to(c1.0, c1.1, c2.0, c2.1, end.0, end.1)
            }
        }
    }
    p
}

// ==========================================================================
// TilingPattern / qt_pattern (graphics.py lines 47-249)
// ==========================================================================

/// Port of `TilingPattern` (graphics.py lines 47-67).
pub struct TilingPattern {
    pub inner: Stream,
    pub paint_type: i64,
    pub w: f64,
    pub h: f64,
    pub matrix: [f64; 6],
    pub resources: Dictionary,
    /// The caller-supplied identity token used to build [`Self::cache_key`]
    /// (Python's `cache_key` constructor parameter) - kept separately so
    /// [`TexturePattern::from_clone`] can rebuild a pattern with the same
    /// identity but a different matrix, matching `clone.cache_key[1]`.
    pub cache_key_seed: String,
    /// Dedup key: mirrors Python's `(class name, cache_key, self.matrix)`
    /// tuple, flattened to a comparable/hashable string (same approach as
    /// `gradients::LinearGradientPattern::cache_key`).
    pub cache_key: String,
}

fn fmt_f64_key(f: f64) -> String {
    format!("{f:.12e}")
}

impl TilingPattern {
    /// Port of `TilingPattern.__init__` (graphics.py lines 49-56).
    pub fn new(
        cache_key_seed: &str,
        matrix: &Matrix,
        w: f64,
        h: f64,
        paint_type: i64,
        compress: bool,
    ) -> Self {
        let mat = matrix.as_tuple();
        let cache_key = format!(
            "TilingPattern|{cache_key_seed}|{}",
            mat.iter()
                .map(|v| fmt_f64_key(*v))
                .collect::<Vec<_>>()
                .join(",")
        );
        TilingPattern {
            inner: Stream::new(compress),
            paint_type,
            w,
            h,
            matrix: mat,
            resources: Dictionary::new(),
            cache_key_seed: cache_key_seed.to_string(),
            cache_key,
        }
    }
}

impl StreamLike for TilingPattern {
    fn stream(&self) -> &Stream {
        &self.inner
    }
    /// Port of `TilingPattern.add_extra_keys` (graphics.py lines 58-67).
    fn extra_keys(&self) -> Vec<(String, PdfObj)> {
        vec![
            ("Type".to_string(), Name::new("Pattern").into()),
            ("PatternType".to_string(), 1i64.into()),
            ("PaintType".to_string(), self.paint_type.into()),
            ("TilingType".to_string(), 1i64.into()),
            ("BBox".to_string(), {
                let mut a = Array::new();
                a.push(0i64);
                a.push(0i64);
                a.push(self.w);
                a.push(self.h);
                a.into()
            }),
            ("XStep".to_string(), self.w.into()),
            ("YStep".to_string(), self.h.into()),
            ("Matrix".to_string(), {
                let mut a = Array::new();
                a.extend(self.matrix.iter().copied());
                a.into()
            }),
            ("Resources".to_string(), self.resources.clone().into()),
        ]
    }
}

/// Port of `QtPattern.qt_patterns` (graphics.py lines 72-221): the
/// hardcoded PDF content-stream bodies for Qt's 13 built-in hatch brush
/// styles (`Dense1Pattern`..`Dense7Pattern`, `HorPattern`, `VerPattern`,
/// `CrossPattern`, `BDiagPattern`, `FDiagPattern`, `DiagCrossPattern`),
/// indexed by `pattern_num - 2` (Qt's `Dense1Pattern` enum value is `2`).
pub const QT_PATTERNS: [&str; 13] = [
    "0 J\n6 w\n[] 0 d\n4 0 m\n4 8 l\n0 4 m\n8 4 l\nS\n",
    "0 J\n2 w\n[6 2] 1 d\n0 0 m\n0 8 l\n8 0 m\n8 8 l\nS\n[] 0 d\n2 0 m\n2 8 l\n6 0 m\n6 8 l\nS\n[6 2] -3 d\n4 0 m\n4 8 l\nS\n",
    "0 J\n2 w\n[6 2] 1 d\n0 0 m\n0 8 l\n8 0 m\n8 8 l\nS\n[2 2] -1 d\n2 0 m\n2 8 l\n6 0 m\n6 8 l\nS\n[6 2] -3 d\n4 0 m\n4 8 l\nS\n",
    "0 J\n2 w\n[2 2] 1 d\n0 0 m\n0 8 l\n8 0 m\n8 8 l\nS\n[2 2] -1 d\n2 0 m\n2 8 l\n6 0 m\n6 8 l\nS\n[2 2] 1 d\n4 0 m\n4 8 l\nS\n",
    "0 J\n2 w\n[2 6] -1 d\n0 0 m\n0 8 l\n8 0 m\n8 8 l\nS\n[2 2] 1 d\n2 0 m\n2 8 l\n6 0 m\n6 8 l\nS\n[2 6] 3 d\n4 0 m\n4 8 l\nS\n",
    "0 J\n2 w\n[2 6] -1 d\n0 0 m\n0 8 l\n8 0 m\n8 8 l\nS\n[2 6] 3 d\n4 0 m\n4 8 l\nS\n",
    "0 J\n2 w\n[2 6] -1 d\n0 0 m\n0 8 l\n8 0 m\n8 8 l\nS\n",
    "1 w\n0 4 m\n8 4 l\nS\n",
    "1 w\n4 0 m\n4 8 l\nS\n",
    "1 w\n4 0 m\n4 8 l\n0 4 m\n8 4 l\nS\n",
    "1 w\n-1 5 m\n5 -1 l\n3 9 m\n9 3 l\nS\n",
    "1 w\n-1 3 m\n5 9 l\n3 -1 m\n9 5 l\nS\n",
    "1 w\n-1 3 m\n5 9 l\n3 -1 m\n9 5 l\n-1 5 m\n5 -1 l\n3 9 m\n9 3 l\nS\n",
];

/// Port of `QtPattern.__init__` (graphics.py lines 223-225). See module
/// doc comment for why this is a free function rather than a wrapper
/// type.
pub fn qt_pattern(pattern_num: i64, matrix: &Matrix) -> TilingPattern {
    let mut tp = TilingPattern::new(&pattern_num.to_string(), matrix, 8.0, 8.0, 2, false);
    let idx = (pattern_num - 2) as usize;
    tp.inner.write(QT_PATTERNS[idx]);
    tp
}

// ==========================================================================
// TexturePattern (graphics.py lines 228-248)
// ==========================================================================

/// Port of `TexturePattern` (graphics.py lines 228-248). Per the porting
/// brief, the Qt-coupled inputs (`pixmap.toImage()`, `cacheKey()`, format
/// check) are replaced with a plain `(image_ref, cache_key, width,
/// height, is_mono)` parameter set - extracting those from a real Qt
/// pixmap/PdfStream (`pdf.add_image(image, cache_key)` in the original)
/// is the caller's job, not this module's.
pub struct TexturePattern {
    pub tiling: TilingPattern,
}

impl TexturePattern {
    /// Port of the `clone is None` branch of `TexturePattern.__init__`
    /// (graphics.py lines 231-242).
    pub fn new(
        image_ref: Reference,
        cache_key: &str,
        width: f64,
        height: f64,
        is_mono: bool,
        matrix: &Matrix,
    ) -> Self {
        let paint_type = if is_mono { 2 } else { 1 };
        let mut tp = TilingPattern::new(cache_key, matrix, width, height, paint_type, false);
        let m = [tp.w, 0.0, 0.0, -tp.h, 0.0, tp.h];
        let mut xobj = Dictionary::new();
        xobj.insert("Texture", image_ref);
        tp.resources.insert("XObject", xobj);
        let toks: Vec<String> = m.iter().map(|v| super::common::pdf_float(*v)).collect();
        tp.inner
            .write_line(format!("{} cm /Texture Do", toks.join(" ")));
        TexturePattern { tiling: tp }
    }

    /// Port of the `clone is not None` branch of `TexturePattern.__init__`
    /// (graphics.py lines 243-248): rebuilds an existing texture pattern
    /// against a new transform matrix, reusing its image/content.
    pub fn from_clone(clone: &TexturePattern, matrix: &Matrix) -> Self {
        let ct = &clone.tiling;
        let mut tp =
            TilingPattern::new(&ct.cache_key_seed, matrix, ct.w, ct.h, ct.paint_type, false);
        if let Some(PdfObj::Dict(xobj)) = ct.resources.get("XObject") {
            tp.resources.insert("XObject", xobj.clone());
        }
        tp.inner.write_raw(ct.inner.getvalue());
        TexturePattern { tiling: tp }
    }
}

impl StreamLike for TexturePattern {
    fn stream(&self) -> &Stream {
        &self.tiling.inner
    }
    fn extra_keys(&self) -> Vec<(String, PdfObj)> {
        self.tiling.extra_keys()
    }
}

// ==========================================================================
// GraphicsState / Graphics (graphics.py lines 251-485): documented gap
// ==========================================================================

/// Plain-data stand-in for `QBrush::style()`'s relevant values (Qt's
/// `Qt::BrushStyle` enum, in Qt's own declaration order - `convert_brush`
/// compares against it with `<=`/`==`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrushStyle {
    NoBrush,
    SolidPattern,
    Dense1,
    Dense2,
    Dense3,
    Dense4,
    Dense5,
    Dense6,
    Dense7,
    Hor,
    Ver,
    Cross,
    BDiag,
    FDiag,
    DiagCross,
    LinearGradientPattern,
    TexturePattern,
}

/// Plain-data stand-in for `QBrush`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlainBrush {
    pub style: BrushStyle,
    pub color: [f64; 4],
}

/// Plain-data stand-in for `QPen`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlainPen {
    pub width: f64,
    pub cosmetic: bool,
    pub cap_style: i64,
    pub join_style: i64,
    pub dash_pattern: Vec<f64>,
    pub dash_offset: f64,
    pub style: i64,
    pub brush: PlainBrush,
}

/// Port of `Brush` (graphics.py line 44).
#[derive(Debug, Clone)]
pub struct Brush {
    pub origin: (f64, f64),
    pub pattern_cache_key: Option<String>,
    pub color: Option<[f64; 3]>,
}

/// Port of `GraphicsState` (graphics.py lines 251-282). Plain data + the
/// Python `__eq__`/`.copy()` methods, which are trivial field
/// comparisons/clones with no Qt-specific behavior of their own - ported
/// for real via `derive`. See module doc comment for why [`Graphics`]'s
/// *methods* (which actually process live paint-engine state) are the
/// documented gap, not this struct.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphicsState {
    pub fill: PlainBrush,
    pub stroke: PlainPen,
    pub opacity: f64,
    pub transform: Matrix,
    pub brush_origin: (f64, f64),
    pub clip_updated: bool,
    pub do_fill: bool,
    pub do_stroke: bool,
}

impl Default for GraphicsState {
    /// Port of `GraphicsState.__init__` (graphics.py lines 256-265).
    fn default() -> Self {
        GraphicsState {
            fill: PlainBrush {
                style: BrushStyle::SolidPattern,
                color: [1.0, 1.0, 1.0, 1.0],
            }, // Qt::white
            stroke: PlainPen {
                width: 0.0,
                cosmetic: true,
                cap_style: 0,
                join_style: 0,
                dash_pattern: Vec::new(),
                dash_offset: 0.0,
                style: 1, // SolidLine
                brush: PlainBrush {
                    style: BrushStyle::SolidPattern,
                    color: [0.0, 0.0, 0.0, 1.0],
                },
            },
            opacity: 1.0,
            transform: Matrix::identity(),
            brush_origin: (0.0, 0.0),
            clip_updated: false,
            do_fill: false,
            do_stroke: true,
        }
    }
}

/// Port of `Graphics` (graphics.py lines 285-485): receives live
/// `QPaintEngine` state-change/draw calls during on-screen HTML
/// rendering and re-emits them as PDF operators via a `PdfStream`. See
/// module doc comment - there is no non-Qt input in this codebase to
/// drive these methods with.
pub struct Graphics {
    pub base_state: GraphicsState,
    pub current_state: GraphicsState,
    pub pending_state: Option<GraphicsState>,
    pub page_width_px: f64,
    pub page_height_px: f64,
}

impl Graphics {
    /// Port of `Graphics.__init__` (graphics.py lines 287-291). Real -
    /// just stores page dimensions and default states.
    pub fn new(page_width_px: f64, page_height_px: f64) -> Self {
        Graphics {
            base_state: GraphicsState::default(),
            current_state: GraphicsState::default(),
            pending_state: None,
            page_width_px,
            page_height_px,
        }
    }

    /// Port of `Graphics.begin` (graphics.py lines 293-294): stores the
    /// live `PdfStream` this `Graphics` will emit operators into.
    pub fn begin(&mut self, _pdf: &mut super::serialize::PdfStream) {
        todo!("placeholder: QPaintEngine-interception logic - requires driving from a live Qt paint engine, out of scope for this port")
    }

    /// Port of `Graphics.update_state` (graphics.py lines 296-319):
    /// merges a `QPaintEngineState`'s dirty flags into `pending_state`.
    pub fn update_state(&mut self, _dirty_flags: u32) {
        todo!("placeholder: QPaintEngine-interception logic - requires driving from a live Qt paint engine, out of scope for this port")
    }

    /// Port of `Graphics.reset` (graphics.py lines 321-323).
    pub fn reset(&mut self) {
        todo!("placeholder: QPaintEngine-interception logic - requires driving from a live Qt paint engine, out of scope for this port")
    }

    /// Port of `Graphics.__call__` (graphics.py lines 325-359): applies
    /// the currently pending state to the PDF (transform/clip/stroke/fill
    /// changes since the last call).
    pub fn apply_pending_state(&mut self) {
        todo!("placeholder: QPaintEngine-interception logic - requires driving from a live Qt paint engine, out of scope for this port")
    }

    /// Port of `Graphics.convert_brush` (graphics.py lines 361-400):
    /// converts a `QBrush` to PDF fill/stroke operators (solid color,
    /// hatch pattern, texture, or gradient).
    pub fn convert_brush(
        &mut self,
        _brush: &PlainBrush,
        _brush_origin: (f64, f64),
        _global_opacity: f64,
    ) -> (Option<[f64; 3]>, f64, Option<String>, bool) {
        todo!("placeholder: QPaintEngine-interception logic - requires driving from a live Qt paint engine, out of scope for this port")
    }

    /// Port of `Graphics.apply_stroke` (graphics.py lines 402-444).
    pub fn apply_stroke(&mut self, _state: &GraphicsState) {
        todo!("placeholder: QPaintEngine-interception logic - requires driving from a live Qt paint engine, out of scope for this port")
    }

    /// Port of `Graphics.apply_fill` (graphics.py lines 447-453).
    pub fn apply_fill(&mut self, _state: &GraphicsState) {
        todo!("placeholder: QPaintEngine-interception logic - requires driving from a live Qt paint engine, out of scope for this port")
    }

    /// Port of `Graphics.resolve_fill` (graphics.py lines 461-485):
    /// works around Qt not updating `brushOrigin` for texture-pattern
    /// (incl. emulated-gradient) fills.
    pub fn resolve_fill(&mut self, _rect_top_left: (f64, f64)) {
        todo!("placeholder: QPaintEngine-interception logic - requires driving from a live Qt paint engine, out of scope for this port")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- convert_path ---------------------------------------------------

    #[test]
    fn convert_path_moveto_lineto() {
        let els = [PathElement::MoveTo(1.0, 2.0), PathElement::LineTo(3.0, 4.0)];
        let p = convert_path(&els);
        assert_eq!(p.ops.len(), 2);
        assert_eq!(p.ops[0], super::super::serialize::PathOp::MoveTo(1.0, 2.0));
        assert_eq!(p.ops[1], super::super::serialize::PathOp::LineTo(3.0, 4.0));
    }

    #[test]
    fn convert_path_curveto() {
        let els = [PathElement::CurveTo {
            c1: (1.0, 1.0),
            c2: (2.0, 2.0),
            end: (3.0, 3.0),
        }];
        let p = convert_path(&els);
        assert_eq!(
            p.ops[0],
            super::super::serialize::PathOp::CurveTo(1.0, 1.0, 2.0, 2.0, 3.0, 3.0)
        );
    }

    #[test]
    fn convert_path_empty_input_gives_empty_path() {
        let p = convert_path(&[]);
        assert!(p.ops.is_empty());
    }

    // ---- TilingPattern / qt_pattern ---------------------------------------

    #[test]
    fn tiling_pattern_extra_keys_shape() {
        let tp = TilingPattern::new("seed", &Matrix::identity(), 8.0, 8.0, 2, false);
        let keys: std::collections::HashMap<String, PdfObj> = tp.extra_keys().into_iter().collect();
        assert_eq!(keys.get("PatternType"), Some(&PdfObj::Int(1)));
        assert_eq!(keys.get("PaintType"), Some(&PdfObj::Int(2)));
        assert_eq!(keys.get("XStep"), Some(&PdfObj::Real(8.0)));
    }

    #[test]
    fn qt_pattern_writes_hatch_content_and_bbox() {
        let tp = qt_pattern(2, &Matrix::identity()); // Dense1Pattern
        let text = String::from_utf8(tp.inner.getvalue().to_vec()).unwrap();
        assert!(text.contains("4 0 m"));
        assert_eq!(tp.w, 8.0);
    }

    #[test]
    fn qt_pattern_last_index_diag_cross() {
        let tp = qt_pattern(14, &Matrix::identity()); // DiagCrossPattern, index 12 (last)
        let text = String::from_utf8(tp.inner.getvalue().to_vec()).unwrap();
        assert!(text.contains("-1 5 m"));
        assert!(text.contains("9 3 l"));
    }

    #[test]
    fn tiling_pattern_cache_key_differs_by_matrix() {
        let tp1 = TilingPattern::new("seed", &Matrix::identity(), 8.0, 8.0, 2, false);
        let tp2 = TilingPattern::new("seed", &Matrix::translate(1.0, 0.0), 8.0, 8.0, 2, false);
        assert_ne!(tp1.cache_key, tp2.cache_key);
    }

    // ---- TexturePattern -------------------------------------------------

    #[test]
    fn texture_pattern_builds_xobject_and_content() {
        let pat = TexturePattern::new(
            Reference::new(3),
            "cache1",
            64.0,
            32.0,
            false,
            &Matrix::identity(),
        );
        assert_eq!(pat.tiling.paint_type, 1);
        let text = String::from_utf8(pat.tiling.inner.getvalue().to_vec()).unwrap();
        assert!(text.contains("cm /Texture Do"));
        assert!(pat.tiling.resources.contains_key("XObject"));
    }

    #[test]
    fn texture_pattern_mono_gets_paint_type_two() {
        let pat = TexturePattern::new(
            Reference::new(3),
            "cache1",
            8.0,
            8.0,
            true,
            &Matrix::identity(),
        );
        assert_eq!(pat.tiling.paint_type, 2);
    }

    #[test]
    fn texture_pattern_from_clone_preserves_seed_reuses_content() {
        let original = TexturePattern::new(
            Reference::new(3),
            "cache1",
            16.0,
            16.0,
            false,
            &Matrix::identity(),
        );
        let cloned = TexturePattern::from_clone(&original, &Matrix::translate(5.0, 5.0));
        assert_eq!(cloned.tiling.cache_key_seed, "cache1");
        assert_eq!(
            cloned.tiling.inner.getvalue(),
            original.tiling.inner.getvalue()
        );
        assert_ne!(cloned.tiling.cache_key, original.tiling.cache_key); // different matrix
        assert!(cloned.tiling.resources.contains_key("XObject"));
    }

    // ---- GraphicsState: real, trivial data ops -----------------------------

    #[test]
    fn graphics_state_default_matches_python_defaults() {
        let s = GraphicsState::default();
        assert_eq!(s.opacity, 1.0);
        assert!(!s.do_fill);
        assert!(s.do_stroke);
        assert_eq!(s.transform, Matrix::identity());
    }

    #[test]
    fn graphics_state_clone_is_independent_copy() {
        let mut s1 = GraphicsState::default();
        let s2 = s1.clone();
        s1.opacity = 0.3;
        assert_eq!(s2.opacity, 1.0);
        assert_ne!(s1, s2);
    }

    // ---- Graphics: documented gap ------------------------------------------

    #[test]
    #[should_panic(expected = "placeholder")]
    fn graphics_update_state_is_a_documented_gap() {
        let mut g = Graphics::new(600.0, 800.0);
        g.update_state(0);
    }

    #[test]
    #[should_panic(expected = "placeholder")]
    fn graphics_apply_pending_state_is_a_documented_gap() {
        let mut g = Graphics::new(600.0, 800.0);
        g.apply_pending_state();
    }
}
