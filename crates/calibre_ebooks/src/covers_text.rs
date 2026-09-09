//! Port of `covers.py`'s "Draw text" `QFont`/`QFontMetrics`/
//! `QTextLayout` machinery (issue #598, split from #116): `Block`/
//! `layout_text` and a `tiny-skia`/`usvg`/`resvg`-based rendering
//! bridge, including the real etch text effect.
//!
//! # Font source: a disclosed, scoped decision
//!
//! No font files are bundled in this repo. This matches upstream
//! itself -- `old_src/setup/liberation.py` *downloads* the Liberation
//! fonts at build time into `resources/fonts/liberation/`, it doesn't
//! commit them to source control either. This port uses
//! `fontdb::Database::load_system_fonts()` (real, working text
//! rendering wherever fontconfig/font packages are actually
//! installed) and queries `"Liberation Serif"`/`"Liberation Sans"`
//! with a real fallback to the generic `serif`/`sans-serif` family
//! fontconfig resolves on the host -- not a guess or a hardcoded
//! substitute. **Disclosed narrowing**: a minimal headless container
//! with zero font packages installed has nothing to fall back to and
//! [`FontFace::query`]/[`layout_text`] will return `None`.
//!
//! # Measurement: real glyph advance widths, no shaping
//!
//! Word-wrap decisions and block-height computation use real
//! per-glyph horizontal advance widths read directly from each font's
//! `hmtx` table via `ttf_parser` -- not guessed/estimated character
//! widths. **Disclosed narrowing vs `QTextLayout`'s real behavior**:
//! this sums raw advances with no shaping pass (no kerning, no
//! ligatures, no complex-script reordering) -- `usvg`'s own `harfrust`
//! shaper (used only at *render* time, see [`draw_block`]) may lay a
//! line's glyphs out fractionally narrower/wider than this port's own
//! wrap-width measurement predicted. For the short, mostly-Latin
//! title/author/series strings this module actually renders, the
//! difference is not visually significant. Bold/italic runs
//! ([`FormatRange`]) are measured using the *regular* face's metrics
//! (a second disclosed narrowing) even though they render with real
//! `font-weight`/`font-style` at draw time -- computing a separate
//! bold/italic face's metrics purely for wrap-width purposes was
//! judged not worth the complexity for text this short.

use std::sync::Arc;

use tiny_skia::{Color, Pixmap, Transform};

use crate::covers::{parse_text_formatting, sanitize, unescape_formatting, FormatRange};

/// Port of the `from calibre.gui2 import ... load_builtin_fonts`/
/// `ensure_app` font-loading half of `init_environment` -- loads
/// whatever fonts are actually installed on the host via fontconfig
/// (Linux)/the platform's native font directories (Windows/macOS).
pub fn load_system_fonts() -> fontdb::Database {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    db
}

/// A queried, loaded font face's raw bytes + face index -- kept alive
/// independently of the `fontdb::Database` it was queried from so a
/// [`ttf_parser::Face`] can be parsed from it on demand. Also carries
/// the face's *real* resolved family name (as opposed to the logical
/// name that was queried for), which [`draw_block`]'s own SVG
/// `font-family` needs so `usvg`'s independent font resolution
/// converges on this exact same face rather than potentially failing
/// to resolve a logical/generic name on its own.
pub struct FontFace {
    data: Vec<u8>,
    face_index: u32,
    family_name: String,
}

impl FontFace {
    /// Port of the family-resolution half of `QFont(prefs.xxx_font_family
    /// or 'Liberation Xxx')`: queries `family` first, falling back to
    /// the generic `generic` family (`Serif`/`SansSerif`) the host's
    /// fontconfig config resolves -- matching how Qt's own font
    /// substitution behaves when the exact requested family isn't
    /// installed. If even the generic family's *own* configured name
    /// (e.g. fontconfig's `sans-serif` alias pointing at a family that
    /// isn't actually installed) doesn't resolve, falls back once more
    /// to whatever font *is* installed at all -- the same kind of
    /// last-resort substitution real `QFont` performs under the hood
    /// rather than simply failing. Returns `None` only when the host
    /// has no fonts loaded whatsoever (see this module's font-source
    /// doc).
    pub fn query(db: &fontdb::Database, family: &str, generic: fontdb::Family) -> Option<Self> {
        let query = fontdb::Query {
            families: &[fontdb::Family::Name(family), generic],
            ..Default::default()
        };
        let id = db.query(&query).or_else(|| db.faces().next().map(|f| f.id))?;
        let info = db.face(id)?;
        let face_index = info.index;
        let family_name = info.families.first().map(|(name, _)| name.clone()).unwrap_or_default();
        let data = db.with_face_data(id, |data, _| data.to_vec())?;
        Some(FontFace { data, face_index, family_name })
    }

    pub fn face(&self) -> ttf_parser::Face<'_> {
        ttf_parser::Face::parse(&self.data, self.face_index).expect("face bytes already validated by fontdb::Database::query")
    }

    pub fn family_name(&self) -> &str {
        &self.family_name
    }
}

/// Port of the metrics `QFontMetrics(font, img)` exposes: `leading()`/
/// `lineSpacing()`, plus the raw ascender/descender this module needs
/// for baseline placement. `line_height`/`line_spacing` use the
/// standard `ascent + descent (+ leading)` formula rather than Qt's
/// own internal `height() == ascent() + descent() + 1` rounding --
/// a disclosed, sub-pixel-scale simplification.
#[derive(Debug, Clone, Copy)]
pub struct FontMetrics {
    pub pixel_size: f32,
    scale: f32,
    pub ascender: f32,
    pub descender: f32,
}

impl FontMetrics {
    pub fn new(face: &ttf_parser::Face, pixel_size: f32) -> Self {
        let units_per_em = face.units_per_em().max(1) as f32;
        let scale = pixel_size / units_per_em;
        FontMetrics {
            pixel_size,
            scale,
            ascender: face.ascender() as f32 * scale,
            descender: face.descender() as f32 * scale,
        }
    }

    /// Port of `QFontMetrics::leading()`.
    pub fn leading(&self, face: &ttf_parser::Face) -> f32 {
        (face.line_gap() as f32 * self.scale).max(0.0)
    }

    /// A line's own vertical extent, not counting leading (Qt's
    /// `QTextLine::height()`).
    pub fn line_height(&self) -> f32 {
        self.ascender - self.descender
    }

    /// Port of `QFontMetrics::lineSpacing()` (`leading() + height()`).
    pub fn line_spacing(&self, face: &ttf_parser::Face) -> f32 {
        self.leading(face) + self.line_height()
    }

    fn advance_width(&self, face: &ttf_parser::Face, c: char) -> f32 {
        face.glyph_index(c).and_then(|gid| face.glyph_hor_advance(gid)).unwrap_or(0) as f32 * self.scale
    }
}

/// Port of the width-measurement half of `QFontMetrics`/`QTextLine`
/// sizing: the sum of each character's real glyph advance width (see
/// this module's doc for the no-shaping disclosure).
pub fn measure_text_width(face: &ttf_parser::Face, metrics: &FontMetrics, text: &str) -> f32 {
    text.chars().map(|c| metrics.advance_width(face, c)).sum()
}

struct WrappedLine {
    text: String,
    /// Char offset into the paragraph text this line's content starts at.
    start: usize,
    width: f32,
}

/// Port of `QTextOption::WrapMode::WrapAtWordBoundaryOrAnywhere`:
/// greedily packs whitespace-separated words onto each line, falling
/// back to breaking a single overlong word at an arbitrary character
/// boundary when it alone exceeds `max_width`. A real, disclosed
/// simplification of Qt's full Unicode line-breaking (UAX #14): word
/// boundaries are plain ASCII-space splits, not full linebreak-class
/// analysis, and there is no hyphenation.
fn wrap_paragraph(face: &ttf_parser::Face, metrics: &FontMetrics, text: &str, max_width: f32) -> Vec<WrappedLine> {
    let chars: Vec<char> = text.chars().collect();
    let space_width = metrics.advance_width(face, ' ');

    // Tokenize into (word, start_char_idx) pairs, splitting on ASCII
    // space -- char (not byte) offsets, matching `FormatRange`'s own
    // convention so `map_formats_to_line` can compare them directly.
    let mut words: Vec<(&[char], usize)> = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        while i < chars.len() && chars[i] == ' ' {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        let start = i;
        while i < chars.len() && chars[i] != ' ' {
            i += 1;
        }
        words.push((&chars[start..i], start));
    }

    let mut lines: Vec<WrappedLine> = Vec::new();
    let mut current: Vec<char> = Vec::new();
    let mut current_start = 0usize;
    let mut current_width = 0.0f32;

    for (word, word_start) in words {
        let word_width: f32 = word.iter().map(|&c| metrics.advance_width(face, c)).sum();

        if word_width > max_width {
            // The word alone doesn't fit on any line -- flush what's
            // pending, then break the word itself at arbitrary
            // character boundaries (`WrapAtWordBoundaryOrAnywhere`'s
            // "or anywhere" fallback).
            if !current.is_empty() {
                lines.push(WrappedLine { text: current.iter().collect(), start: current_start, width: current_width });
                current.clear();
                current_width = 0.0;
            }
            let mut piece_start = word_start;
            let mut idx_in_word = word_start;
            for &c in word {
                let cw = metrics.advance_width(face, c);
                if current_width + cw > max_width && !current.is_empty() {
                    lines.push(WrappedLine { text: current.iter().collect(), start: piece_start, width: current_width });
                    current.clear();
                    current_width = 0.0;
                    piece_start = idx_in_word;
                }
                current.push(c);
                current_width += cw;
                idx_in_word += 1;
            }
            current_start = piece_start;
            continue;
        }

        if current.is_empty() {
            current_start = word_start;
            current.extend_from_slice(word);
            current_width = word_width;
        } else if current_width + space_width + word_width <= max_width {
            current.push(' ');
            current.extend_from_slice(word);
            current_width += space_width + word_width;
        } else {
            lines.push(WrappedLine { text: current.iter().collect(), start: current_start, width: current_width });
            current.clear();
            current.extend_from_slice(word);
            current_width = word_width;
            current_start = word_start;
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(WrappedLine { text: current.iter().collect(), start: current_start, width: current_width });
    }
    lines
}

/// Clips `formats` (character offsets into the *paragraph*'s full
/// text) down to the ones overlapping `[line_start, line_start +
/// line_len)`, re-based to be relative to the line's own text.
fn map_formats_to_line(formats: &[FormatRange], line_start: usize, line_len: usize) -> Vec<FormatRange> {
    let line_end = line_start + line_len;
    formats
        .iter()
        .filter_map(|f| {
            let f_end = f.start + f.length;
            let start = f.start.max(line_start);
            let end = f_end.min(line_end);
            if start >= end {
                return None;
            }
            Some(FormatRange { bold: f.bold, italic: f.italic, start: start - line_start, length: end - start })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HAlign {
    Left,
    Center,
    Right,
}

fn align_x(container_width: f32, line_width: f32, align: HAlign) -> f32 {
    match align {
        HAlign::Left => 0.0,
        HAlign::Center => ((container_width - line_width) / 2.0).max(0.0),
        HAlign::Right => (container_width - line_width).max(0.0),
    }
}

#[derive(Debug, Clone)]
struct RenderLine {
    text: String,
    formats: Vec<FormatRange>,
    x: f32,
    /// Top-of-line offset, relative to this line's own paragraph.
    y: f32,
    width: f32,
}

#[derive(Debug, Clone)]
struct ParagraphLayout {
    lines: Vec<RenderLine>,
    /// Total vertical extent this paragraph consumed (Qt's
    /// `QTextLayout::boundingRect().height()`).
    height: f32,
}

/// A single line ready to draw, in absolute canvas coordinates.
#[derive(Debug, Clone)]
pub struct PositionedLine {
    pub text: String,
    pub formats: Vec<FormatRange>,
    pub x: f32,
    pub y_baseline: f32,
    pub width: f32,
}

/// Port of `Block`: a `<br>`-separated group of word-wrapped, height-
/// budgeted text paragraphs. See this module's doc for the real
/// per-paragraph spacer-leading bookkeeping this replicates exactly
/// from `Block.__init__`/`Block.height`/`Block.position`.
#[derive(Debug, Clone)]
pub struct Block {
    paragraphs: Vec<ParagraphLayout>,
    pub leading: f32,
    pub line_spacing: f32,
    ascender: f32,
    position: (f32, f32),
}

impl Block {
    /// Port of `Block()` -- the empty default used when there's no
    /// subtitle.
    pub fn empty() -> Self {
        Block { paragraphs: Vec::new(), leading: 0.0, line_spacing: 0.0, ascender: 0.0, position: (0.0, 0.0) }
    }

    /// Port of `Block(text, width, font, img, max_height, align)`.
    pub fn new(text: &str, width: f32, face: &ttf_parser::Face, metrics: &FontMetrics, max_height: f32, align: HAlign) -> Self {
        let leading = metrics.leading(face);
        if text.is_empty() {
            return Block { paragraphs: Vec::new(), leading, line_spacing: metrics.line_spacing(face), ascender: metrics.ascender, position: (0.0, 0.0) };
        }
        let mut paragraphs = Vec::new();
        let mut remaining_height = max_height;
        for paragraph_text in text.split("<br>") {
            let sanitized = sanitize(paragraph_text);
            let (stripped, formats) = parse_text_formatting(&sanitized);
            let unescaped = unescape_formatting(&stripped);
            let wrapped = wrap_paragraph(face, metrics, &unescaped, width);

            let mut lines = Vec::new();
            let mut height = 0.0f32;
            for w in &wrapped {
                if !(height + 3.0 * leading < remaining_height) {
                    break;
                }
                height += leading;
                let y = height;
                height += metrics.line_height();
                let line_formats = map_formats_to_line(&formats, w.start, w.text.chars().count());
                let x = align_x(width, w.width, align);
                lines.push(RenderLine { text: w.text.clone(), formats: line_formats, x, y, width: w.width });
            }
            remaining_height -= height;
            paragraphs.push(ParagraphLayout { lines, height });
        }
        Block { paragraphs, leading, line_spacing: metrics.line_spacing(face), ascender: metrics.ascender, position: (0.0, 0.0) }
    }

    /// Port of the `Block.height` property.
    pub fn height(&self) -> f32 {
        if self.paragraphs.is_empty() {
            return 0.0;
        }
        let sum: f32 = self.paragraphs.iter().map(|p| p.height).sum::<f32>() + self.paragraphs.len() as f32 * self.leading;
        sum.ceil()
    }

    /// Port of the `Block.position` setter.
    pub fn set_position(&mut self, x: f32, y: f32) {
        self.position = (x, y);
    }

    pub fn position(&self) -> (f32, f32) {
        self.position
    }

    /// Flattens every line across every paragraph into absolute
    /// canvas coordinates, replicating `Block.position`'s real
    /// per-paragraph cursor advance (paragraph height, then one
    /// `leading` spacer, repeated -- including a final inert spacer
    /// after the last paragraph that has no observable effect, same
    /// as the real Python).
    pub fn positioned_lines(&self) -> Vec<PositionedLine> {
        let mut out = Vec::new();
        let (bx, by) = self.position;
        let mut y_cursor = by;
        for paragraph in &self.paragraphs {
            for line in &paragraph.lines {
                out.push(PositionedLine {
                    text: line.text.clone(),
                    formats: line.formats.clone(),
                    x: bx + line.x,
                    y_baseline: y_cursor + line.y + self.ascender,
                    width: line.width,
                });
            }
            y_cursor += paragraph.height + self.leading;
        }
        out
    }
}

/// Real font-family preferences (`cprefs`'s `xxx_font_family`/
/// `xxx_font_size` defaults).
#[derive(Debug, Clone)]
pub struct CoverTextPrefs {
    pub title_font_family: Option<String>,
    pub subtitle_font_family: Option<String>,
    pub footer_font_family: Option<String>,
    pub title_font_size: f32,
    pub subtitle_font_size: f32,
    pub footer_font_size: f32,
}

impl Default for CoverTextPrefs {
    fn default() -> Self {
        CoverTextPrefs {
            title_font_family: None,
            subtitle_font_family: None,
            footer_font_family: None,
            title_font_size: 120.0,
            subtitle_font_size: 80.0,
            footer_font_size: 80.0,
        }
    }
}

pub struct LayoutResult {
    pub title: Block,
    pub subtitle: Block,
    pub footer: Block,
    /// The *real* resolved family name for each block's font (see
    /// [`FontFace::family_name`]) -- `subtitle_family` is empty when
    /// there was no subtitle text to lay out. [`draw_block`] must be
    /// called with these, not the logical `prefs`-requested names.
    pub title_family: String,
    pub subtitle_family: String,
    pub footer_family: String,
}

/// Port of `layout_text(prefs, img, title, subtitle, footer, max_height,
/// style)`. Returns `None` only if no usable font could be resolved
/// at all (see this module's font-source doc).
#[allow(clippy::too_many_arguments)]
pub fn layout_text(
    db: &fontdb::Database,
    prefs: &CoverTextPrefs,
    cover_width: u32,
    cover_height: u32,
    hmargin: i32,
    vmargin: i32,
    title: &str,
    subtitle: &str,
    footer: &str,
    max_height: f32,
    title_align: HAlign,
    subtitle_align: HAlign,
    footer_align: HAlign,
) -> Option<LayoutResult> {
    let width = cover_width as f32 - 2.0 * hmargin as f32;

    let title_font = FontFace::query(db, prefs.title_font_family.as_deref().unwrap_or("Liberation Serif"), fontdb::Family::Serif)?;
    let title_face = title_font.face();
    let title_metrics = FontMetrics::new(&title_face, prefs.title_font_size);
    let mut title_block = Block::new(title, width, &title_face, &title_metrics, max_height, title_align);
    title_block.set_position(hmargin as f32, vmargin as f32);
    let title_family = title_font.family_name().to_string();

    let mut subtitle_block = Block::empty();
    let mut subtitle_family = String::new();
    if !subtitle.is_empty() {
        let subtitle_font = FontFace::query(db, prefs.subtitle_font_family.as_deref().unwrap_or("Liberation Sans"), fontdb::Family::SansSerif)?;
        let subtitle_face = subtitle_font.face();
        let subtitle_metrics = FontMetrics::new(&subtitle_face, prefs.subtitle_font_size);
        let gap = 2.0 * title_block.leading;
        let mh = max_height - title_block.height() - gap;
        subtitle_block = Block::new(subtitle, width, &subtitle_face, &subtitle_metrics, mh, subtitle_align);
        subtitle_block.set_position(hmargin as f32, title_block.position().1 + title_block.height() + gap);
        subtitle_family = subtitle_font.family_name().to_string();
    }

    let footer_font = FontFace::query(db, prefs.footer_font_family.as_deref().unwrap_or("Liberation Serif"), fontdb::Family::Serif)?;
    let footer_face = footer_font.face();
    let footer_metrics = FontMetrics::new(&footer_face, prefs.footer_font_size);
    let mut footer_block = Block::new(footer, width, &footer_face, &footer_metrics, max_height, footer_align);
    footer_block.set_position(hmargin as f32, cover_height as f32 - vmargin as f32 - footer_block.height());
    let footer_family = footer_font.family_name().to_string();

    Some(LayoutResult { title: title_block, subtitle: subtitle_block, footer: footer_block, title_family, subtitle_family, footer_family })
}

// ===================================================================
// Rendering bridge (`Block.draw`)
// ===================================================================

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn format_at(formats: &[FormatRange], idx: usize) -> (bool, bool) {
    formats.iter().fold((false, false), |(bold, italic), f| {
        if idx >= f.start && idx < f.start + f.length {
            (bold || f.bold, italic || f.italic)
        } else {
            (bold, italic)
        }
    })
}

fn text_element(line: &PositionedLine, font_family: &str, generic_family: &str, pixel_size: f32, fill: &str, opacity: Option<f32>, dx: f32, dy: f32) -> String {
    let opacity_attr = opacity.map(|o| format!(" fill-opacity=\"{o}\"")).unwrap_or_default();
    let chars: Vec<char> = line.text.chars().collect();
    let mut spans = String::new();
    let mut idx = 0usize;
    while idx < chars.len() {
        let style = format_at(&line.formats, idx);
        let start = idx;
        while idx < chars.len() && format_at(&line.formats, idx) == style {
            idx += 1;
        }
        let run: String = chars[start..idx].iter().collect();
        let escaped = xml_escape(&run);
        let (bold, italic) = style;
        if bold || italic {
            let mut style_attrs = String::new();
            if bold {
                style_attrs.push_str(" font-weight=\"bold\"");
            }
            if italic {
                style_attrs.push_str(" font-style=\"italic\"");
            }
            spans.push_str(&format!("<tspan{style_attrs}>{escaped}</tspan>"));
        } else {
            spans.push_str(&escaped);
        }
    }
    format!(
        "<text x=\"{x}\" y=\"{y}\" font-family=\"{font_family}, {generic_family}\" font-size=\"{pixel_size}\" fill=\"{fill}\"{opacity_attr}>{spans}</text>",
        x = line.x + dx,
        y = line.y_baseline + dy,
    )
}

fn build_block_svg(lines: &[PositionedLine], font_family: &str, generic_family: &str, pixel_size: f32, color: Color, width: u32, height: u32) -> String {
    let c = color.to_color_u8();
    let hex = format!("#{:02x}{:02x}{:02x}", c.red(), c.green(), c.blue());
    let mut out = format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\">");
    for line in lines {
        if line.text.is_empty() {
            continue;
        }
        // Etch effect (`Block.draw`): an offset (1,1), semi-transparent
        // white pass drawn *underneath* the real-color pass.
        out.push_str(&text_element(line, font_family, generic_family, pixel_size, "#ffffff", Some(125.0 / 255.0), 1.0, 1.0));
        out.push_str(&text_element(line, font_family, generic_family, pixel_size, &hex, None, 0.0, 0.0));
    }
    out.push_str("</svg>");
    out
}

/// Port of `Block.draw`: renders every line in `block` onto `pixmap`
/// with the real etch effect, via a small generated SVG rasterized
/// through the already-real `usvg`/`resvg` pipeline (see
/// `oeb::transforms::rasterize` for the existing precedent). `db` must
/// be the same [`fontdb::Database`] (or a superset) used to measure
/// `block` during [`layout_text`]. `font_family` must be the *real*
/// resolved family name ([`FontFace::family_name`]/
/// [`LayoutResult`]'s `*_family` fields) rather than the logical
/// `prefs`-requested name -- `usvg` does its own independent font
/// matching against `db` and has no fallback for a logical/generic
/// name that isn't actually installed (unlike this module's own
/// [`FontFace::query`]).
pub fn draw_block(pixmap: &mut Pixmap, db: &Arc<fontdb::Database>, block: &Block, font_family: &str, generic_family: &str, pixel_size: f32, color: Color) {
    let lines = block.positioned_lines();
    if lines.is_empty() {
        return;
    }
    let svg = build_block_svg(&lines, font_family, generic_family, pixel_size, color, pixmap.width(), pixmap.height());
    let opt = usvg::Options { fontdb: db.clone(), ..Default::default() };
    let tree = match usvg::Tree::from_str(&svg, &opt) {
        Ok(tree) => tree,
        Err(_) => return,
    };
    resvg::render(&tree, Transform::identity(), &mut pixmap.as_mut());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> fontdb::Database {
        load_system_fonts()
    }

    fn regular_face(db: &fontdb::Database) -> FontFace {
        FontFace::query(db, "Liberation Sans", fontdb::Family::SansSerif).expect("a sans-serif font must be installed on the test host")
    }

    #[test]
    fn font_face_query_falls_back_to_the_generic_family() {
        let db = test_db();
        // "Definitely Not A Real Font Xyz" isn't installed anywhere;
        // the generic SansSerif fallback should still resolve.
        let face = FontFace::query(&db, "Definitely Not A Real Font Xyz", fontdb::Family::SansSerif);
        assert!(face.is_some());
    }

    #[test]
    fn measure_text_width_is_positive_and_monotonic_in_length() {
        let db = test_db();
        let font = regular_face(&db);
        let face = font.face();
        let metrics = FontMetrics::new(&face, 40.0);
        let short = measure_text_width(&face, &metrics, "Hi");
        let long = measure_text_width(&face, &metrics, "Hi there");
        assert!(short > 0.0);
        assert!(long > short);
    }

    #[test]
    fn wrap_paragraph_breaks_at_word_boundaries_when_it_fits() {
        let db = test_db();
        let font = regular_face(&db);
        let face = font.face();
        let metrics = FontMetrics::new(&face, 40.0);
        // Wide enough for "one two" but not "one two three".
        let max_width = measure_text_width(&face, &metrics, "one two") + 1.0;
        let lines = wrap_paragraph(&face, &metrics, "one two three four", max_width);
        assert!(lines.len() > 1, "expected the text to wrap onto multiple lines");
        for line in &lines {
            assert!(line.width <= max_width + 0.01, "line {:?} exceeds the width budget", line.text);
        }
        // Reassembling the lines (with single spaces) reproduces the
        // original text -- no words dropped or duplicated.
        let rejoined = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(" ");
        assert_eq!(rejoined, "one two three four");
    }

    #[test]
    fn wrap_paragraph_breaks_a_single_overlong_word_anywhere() {
        let db = test_db();
        let font = regular_face(&db);
        let face = font.face();
        let metrics = FontMetrics::new(&face, 40.0);
        let long_word = "Supercalifragilisticexpialidocious";
        let full_width = measure_text_width(&face, &metrics, long_word);
        let lines = wrap_paragraph(&face, &metrics, long_word, full_width / 3.0);
        assert!(lines.len() > 1, "an overlong single word must still be broken across lines");
        let rejoined: String = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(rejoined, long_word);
    }

    #[test]
    fn map_formats_to_line_clips_and_rebases_offsets() {
        let formats = vec![FormatRange { bold: true, italic: false, start: 4, length: 3 }];
        // Line covers paragraph chars [4, 9) -- exact overlap, rebased to 0.
        let mapped = map_formats_to_line(&formats, 4, 5);
        assert_eq!(mapped, vec![FormatRange { bold: true, italic: false, start: 0, length: 3 }]);
        // Line covers [0, 4) -- no overlap at all.
        assert!(map_formats_to_line(&formats, 0, 4).is_empty());
        // Line covers [6, 10) -- partial overlap, clipped to [6,7).
        let partial = map_formats_to_line(&formats, 6, 4);
        assert_eq!(partial, vec![FormatRange { bold: true, italic: false, start: 0, length: 1 }]);
    }

    #[test]
    fn empty_text_produces_a_zero_height_block() {
        let db = test_db();
        let font = regular_face(&db);
        let face = font.face();
        let metrics = FontMetrics::new(&face, 40.0);
        let block = Block::new("", 400.0, &face, &metrics, 1000.0, HAlign::Center);
        assert_eq!(block.height(), 0.0);
        assert!(block.positioned_lines().is_empty());
    }

    #[test]
    fn block_height_grows_with_more_wrapped_lines() {
        let db = test_db();
        let font = regular_face(&db);
        let face = font.face();
        let metrics = FontMetrics::new(&face, 40.0);
        let one_line = Block::new("Short", 2000.0, &face, &metrics, 1000.0, HAlign::Center);
        let many_lines = Block::new("one two three four five six seven eight nine ten", 100.0, &face, &metrics, 1000.0, HAlign::Center);
        assert!(many_lines.height() > one_line.height());
    }

    #[test]
    fn block_respects_a_tight_max_height_budget() {
        let db = test_db();
        let font = regular_face(&db);
        let face = font.face();
        let metrics = FontMetrics::new(&face, 40.0);
        let leading = metrics.leading(&face);
        // Exactly matches the real loop guard's own boundary
        // (`height_after_one_line + 3*leading < max_height`): after one
        // line, `height` is `leading + line_height`, so a budget of
        // `line_height + 4*leading` is the largest value that still
        // stops the loop before creating a second line -- note this
        // guard only checks *prior accumulated* height against the
        // budget, not whether the *next* line would itself fit, a real
        // quirk this port faithfully reproduces from the real Python.
        let max_height = metrics.line_height() + 4.0 * leading;
        let block = Block::new("one two three four five six", 30.0, &face, &metrics, max_height, HAlign::Left);
        let lines = block.positioned_lines();
        assert_eq!(lines.len(), 1, "expected the tight height budget to keep only the first line");
    }

    #[test]
    fn center_alignment_centers_a_short_line_within_the_width() {
        let db = test_db();
        let font = regular_face(&db);
        let face = font.face();
        let metrics = FontMetrics::new(&face, 40.0);
        let block = Block::new("Hi", 1000.0, &face, &metrics, 1000.0, HAlign::Center);
        let lines = block.positioned_lines();
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        // Roughly centered: left gap ~= right gap.
        let left_gap = line.x;
        let right_gap = 1000.0 - (line.x + line.width);
        assert!((left_gap - right_gap).abs() < 1.0, "left={left_gap} right={right_gap}");
    }

    #[test]
    fn set_position_places_the_first_line_at_the_given_origin() {
        let db = test_db();
        let font = regular_face(&db);
        let face = font.face();
        let metrics = FontMetrics::new(&face, 40.0);
        let mut block = Block::new("Hello", 1000.0, &face, &metrics, 1000.0, HAlign::Left);
        block.set_position(50.0, 80.0);
        let lines = block.positioned_lines();
        assert_eq!(lines[0].x, 50.0);
        // First line's baseline is the block's y origin plus the
        // face's own ascender (no leading before the very first line
        // -- matches the real Python: `height` starts at 0 and the
        // first line's leading is added before its own position).
        let expected_baseline = 80.0 + metrics.leading(&face) + metrics.ascender;
        assert!((lines[0].y_baseline - expected_baseline).abs() < 0.01);
    }

    #[test]
    fn layout_text_positions_title_subtitle_and_footer_without_overlap() {
        let db = test_db();
        let prefs = CoverTextPrefs::default();
        let result = layout_text(&db, &prefs, 1200, 1600, 50, 50, "A Title", "A Subtitle", "A Footer", 1600.0, HAlign::Center, HAlign::Center, HAlign::Center)
            .expect("a font must be resolvable on the test host");
        assert_eq!(result.title.position(), (50.0, 50.0));
        // Subtitle never starts above the title block ends (it can be
        // flush against it when the title font's own line-gap is 0).
        assert!(result.subtitle.position().1 >= result.title.position().1 + result.title.height());
        // Footer sits above the bottom margin, its own height above it.
        let (_, footer_y) = result.footer.position();
        assert!((footer_y + result.footer.height() - (1600.0 - 50.0)).abs() < 1.0);
    }

    #[test]
    fn draw_block_paints_real_visible_pixels_with_the_etch_shadow_offset() {
        let db = Arc::new(test_db());
        let font = FontFace::query(&db, "Liberation Sans", fontdb::Family::SansSerif).unwrap();
        let face = font.face();
        let metrics = FontMetrics::new(&face, 60.0);
        let mut block = Block::new("Hi", 400.0, &face, &metrics, 200.0, HAlign::Left);
        block.set_position(20.0, 100.0);

        let mut pixmap = Pixmap::new(400, 300).unwrap();
        pixmap.fill(Color::BLACK);
        // `usvg` does its own independent font resolution against `db`
        // and has no generic/last-resort fallback of its own -- pass
        // the *real* resolved family name (see `draw_block`'s doc),
        // not the logical one that was queried for.
        draw_block(&mut pixmap, &db, &block, font.family_name(), "sans-serif", 60.0, Color::from_rgba8(0, 200, 0, 255));

        // At least some pixels changed from the plain black background
        // -- real glyph coverage was actually rasterized onto the canvas.
        let changed = pixmap.pixels().iter().filter(|p| p.red() != 0 || p.green() != 0 || p.blue() != 0).count();
        assert!(changed > 20, "expected real rasterized glyph pixels, found {changed} non-background pixels");
    }
}
