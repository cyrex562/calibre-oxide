//! Port of `old_src/src/calibre/ebooks/pdf/reflow.py` (2,092 lines).
//!
//! This is calibre's own PDF text-reflow/layout-reconstruction algorithm:
//! it consumes the XML that `pdftohtml -xml` (poppler-utils) produces -
//! `<fontspec>`/`<page>`/`<text>`/`<image>` elements carrying absolute
//! pixel coordinates - and reconstructs reading order, paragraphs,
//! indentation, alignment and (heuristically) heading levels, emitting
//! clean XHTML. Pure geometry/statistics, no native/Qt dependency, which
//! is exactly what makes this genuinely portable (unlike `html_writer.py`
//! a few doors down, which drives an actual Qt browser engine).
//!
//! Class-for-class mapping to the Python module:
//!
//! | Python | Rust |
//! | --- | --- |
//! | `Font` | [`Font`] |
//! | `Element` (base of `Image`/`Text`) | fields folded directly into [`Image`]/[`Text`]; `starts_block`/`block_style` are dead fields in the original (set in `__init__`, never read or written again) and are dropped rather than carried forward as unused noise |
//! | `DocStats` | [`DocStats`] |
//! | `Image` | [`Image`] |
//! | `Text` | [`Text`] |
//! | `Paragraph` | [`Paragraph`] (dead code upstream too: defined, never instantiated) |
//! | `FontSizeStats` | [`FontSizeStats`] |
//! | `Interval` | [`Interval`] |
//! | `Column` | [`Column`] |
//! | `Box` | [`HtmlBox`] (`Box` collides with `std::boxed::Box`) |
//! | `ImageBox` | [`ImageBox`] |
//! | `Region` | [`Region`] |
//! | `Page` | [`Page`] |
//! | `PDFDocument` | [`PdfDocument`] |
//!
//! Note on `Column`/`Box`/`ImageBox`/`Region` and `Page::sort_into_columns`/
//! `find_elements_in_row_of`/`coalesce_regions`/`PDFDocument::linearize`:
//! in the *original Python*, `Page.find_margins` and `Page.second_pass`
//! both `return` before reaching the code that would build `self.regions`
//! (it is explicitly labelled `#### NOT IMPLEMENTED ####` in the source),
//! and `PDFDocument.linearize` is never called (`__init__` has it
//! commented out, calling `self.render()` directly instead). So this
//! machinery is unreachable dead code in upstream calibre today - ported
//! here faithfully anyway per this port's scope (it's real, well-defined
//! geometry, and may be wired up later), with its own unit tests since
//! `PdfDocument`'s real pipeline never exercises it.
//!
//! Numeric note: Python's `round()` uses round-half-to-even; this port
//! uses `f64::round()` (round-half-away-from-zero) throughout for
//! simplicity. The two disagree only on exact `.5` ties, which essentially
//! never occur for coordinates coming out of a PDF renderer.

use indexmap::IndexMap;
use regex::Regex;
use roxmltree::{Document, Node};
use std::collections::HashMap;
use std::sync::OnceLock;

use super::utils::encode_for_xml;

// ==========================================================================
// Global constants affecting formatting decisions (reflow.py lines 15-86)
// ==========================================================================

/// How many pages to scan when finding header/footer automatically.
pub const PAGE_SCAN_COUNT: usize = 20;
/// How many lines (from top/bottom) to scan when finding header/footer.
pub const LINE_SCAN_COUNT: usize = 2;
/// Character-width multiple apart for two strings to coalesce into one line.
pub const COALESCE_FACTOR: f64 = 20.0;
/// Dither allowed in bottom-of-character overlap when checking same line.
pub const BOTTOM_FACTOR: f64 = 2.0;
/// Overlap-vs-line-height factor beyond which lines are considered distinct.
pub const HEIGHT_FACTOR: f64 = 1.5;
/// Fraction of text height two strings' bottoms may differ by and still coalesce.
pub const LINE_FACTOR: f64 = 0.2;
/// Percent of the last line that must be filled for a long word not to force a new line.
pub const LAST_LINE_PERCENT: f64 = 60.0;
/// Margin (in lines) allowed when deciding a page finished early (orphan avoidance).
pub const ORPHAN_LINES: f64 = 5.0;
/// Multiplies inter-line gap to decide a paragraph break is likely valid.
pub const PARA_FACTOR: f64 = 1.8;
/// Multiplies paragraph gap to decide this is a section break, not a paragraph break.
pub const SECTION_FACTOR: f64 = 1.3;
/// Multiplies average line height when detecting columns.
pub const YFUZZ: f64 = 1.5;
/// Plus-or-minus dither allowed on left (and other) margins.
pub const LEFT_WAVER: f64 = 2.0;
/// Amount left margin must exceed right by for text to be considered right-aligned.
pub const RIGHT_FACTOR: f64 = 1.8;
/// Percentage amount left/right margins can differ and still be considered centered.
pub const CENTER_FACTOR: f64 = 0.15;
/// How near text-right needs to be to the right margin to count as right-aligned.
pub const RIGHT_FLOAT_FACTOR: f64 = 0.05;
/// How near pixel values must be to be considered the same space.
pub const SAME_SPACE: f64 = 3.0;
/// How near pixel values must be to be considered the same indent.
pub const SAME_INDENT: f64 = 2.0;

/// Round like Python's `round()` for the common (non-banker's-tie) case.
/// See the module-level numeric note.
fn py_round(x: f64) -> f64 {
    x.round()
}

/// Round to `ndigits` decimal places, as `round(x, ndigits)` in Python.
fn py_round_to(x: f64, ndigits: i32) -> f64 {
    let factor = 10f64.powi(ndigits);
    (x * factor).round() / factor
}

// ==========================================================================
// Errors
// ==========================================================================

/// Errors surfaced while parsing `pdftohtml -xml` output into a
/// [`PdfDocument`]. Kept distinct from [`crate::pdf::utils::ReflowException`]
/// (that one mirrors the C++ `utils.h` type; this one is this port's own
/// "malformed input" channel, per `docs/FAULT_TOLERANCE.md` - no
/// `.unwrap()`/`.expect()` on parsing untrusted input).
#[derive(Debug, thiserror::Error)]
pub enum ReflowError {
    #[error("invalid XML in pdftohtml output: {0}")]
    Xml(#[from] roxmltree::Error),
    #[error("<{tag}> is missing required attribute `{attr}`")]
    MissingAttribute { tag: String, attr: String },
    #[error("<{tag}> attribute `{attr}` is not a valid number: {value:?}")]
    InvalidNumber {
        tag: String,
        attr: String,
        value: String,
    },
    #[error("<text> references unknown font id `{0}`")]
    UnknownFont(String),
}

fn attr_f64(node: &Node, tag: &str, attr: &str) -> Result<f64, ReflowError> {
    let value = node
        .attribute(attr)
        .ok_or_else(|| ReflowError::MissingAttribute {
            tag: tag.to_string(),
            attr: attr.to_string(),
        })?;
    value
        .parse::<f64>()
        .map_err(|_| ReflowError::InvalidNumber {
            tag: tag.to_string(),
            attr: attr.to_string(),
            value: value.to_string(),
        })
}

// ==========================================================================
// A tiny message sink, mirroring `calibre.utils.logging`'s `log.debug`/
// `log.warn`. See `crates/calibre_ebooks/src/mobi/mod.rs`'s `MobiLog` for
// the established convention this follows.
// ==========================================================================

#[derive(Debug, Default, Clone)]
pub struct ReflowLog {
    pub messages: Vec<String>,
}

impl ReflowLog {
    pub fn debug(&mut self, msg: impl Into<String>) {
        self.messages.push(format!("DEBUG: {}", msg.into()));
    }

    pub fn warn(&mut self, msg: impl Into<String>) {
        self.messages.push(format!("WARNING: {}", msg.into()));
    }
}

/// Options consulted by the reflow algorithm. Port of the subset of
/// `calibre.ebooks.conversion.plugins.pdf_input.PDFInput`'s
/// `OptionRecommendation`s that `reflow.py` actually reads
/// (`opts.no_images`, `opts.unwrap_factor`, `opts.pdf_header_skip`,
/// `opts.pdf_footer_skip`, `opts.pdf_header_regex`, `opts.pdf_footer_regex`,
/// `opts.verbose`), with the same recommended defaults.
#[derive(Debug, Clone)]
pub struct ReflowOpts {
    pub verbose: i32,
    pub no_images: bool,
    pub unwrap_factor: f64,
    /// Negative: auto-detect. Zero: don't remove. Positive: pixel threshold.
    pub pdf_header_skip: f64,
    /// Negative: auto-detect. Zero: don't remove. Positive: pixel threshold.
    pub pdf_footer_skip: f64,
    pub pdf_header_regex: String,
    pub pdf_footer_regex: String,
}

impl Default for ReflowOpts {
    fn default() -> Self {
        Self {
            verbose: 0,
            no_images: false,
            unwrap_factor: 0.45,
            pdf_header_skip: -1.0,
            pdf_footer_skip: -1.0,
            pdf_header_regex: String::new(),
            pdf_footer_regex: String::new(),
        }
    }
}

/// Monotonic id generator. Port of `idc = iter(range(sys.maxsize))`
/// (reflow.py line 1437), threaded through every `Text`/`Image`/`Paragraph`
/// constructor the way the Python `idc` iterator is.
#[derive(Debug, Default)]
pub struct IdGen(u64);

impl IdGen {
    pub fn new() -> Self {
        Self(0)
    }

    pub fn next_id(&mut self) -> u64 {
        let id = self.0;
        self.0 += 1;
        id
    }
}

/// A key type letting `f64` pixel values be used as hash-map keys, needed
/// because Python freely uses floats as dict keys throughout this module
/// (`tops[top]`, `indents[left]`, ...). Uses the raw bit pattern, which is
/// fine here: every value that flows through this key type originates from
/// a finite, already-`round()`ed pixel coordinate - never NaN, never the
/// result of arithmetic that would produce distinct bit patterns for the
/// "same" value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FKey(u64);

impl FKey {
    pub fn new(v: f64) -> Self {
        FKey(v.to_bits())
    }

    pub fn value(self) -> f64 {
        f64::from_bits(self.0)
    }
}

// ==========================================================================
// adjacent_quotes (reflow.py lines 89-106)
// ==========================================================================

fn regex_last_nonspace() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^.*([^ ])\s*$").expect("static regex"))
}

fn regex_first_nonspace() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*([^ ])").expect("static regex"))
}

/// Does one string end with a closing quote and the next start with an
/// opening quote? Port of `adjacent_quotes` (reflow.py lines 89-106).
pub fn adjacent_quotes(first_string: &str, second_string: &str) -> bool {
    let last_char = regex_last_nonspace()
        .captures(first_string)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str())
        .unwrap_or(" ");
    let first_char = regex_first_nonspace()
        .captures(second_string)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str())
        .unwrap_or(" ");

    (last_char == "\"" && first_char == "\"")
        || (last_char == "\u{2019}" && first_char == "\u{2018}")
        || (last_char == "\u{201d}" && first_char == "\u{201c}")
}

// ==========================================================================
// XML text handling helpers used while building `Text::raw`/`text_as_string`
// ==========================================================================

/// Port of Python's `html.escape(s)` (default `quote=True`): escapes
/// `&`, `<`, `>`, `"` and `'`. Used only for the very first, direct text run
/// of a `<text>` element (`text.text` in lxml terms) - matching
/// `self.raw = escape(text.text) if text.text else ''` (reflow.py line 206).
fn html_escape_py(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

/// XML text-content escaping (`&`, `<`, `>` only - no quotes). Used for
/// everything that gets serialized through lxml's `etree.tostring(...,
/// method='xml')`, i.e. tails and nested element text, as opposed to the
/// `html.escape()`d leading text handled by [`html_escape_py`].
fn xml_text_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// Concatenate all descendant text content of `node`, ignoring tags.
/// Port of `etree.tostring(text, method='text', encoding='unicode')`
/// (reflow.py line 205).
fn text_content(node: Node) -> String {
    let mut out = String::new();
    for child in node.children() {
        if child.is_text() {
            out.push_str(child.text().unwrap_or(""));
        } else if child.is_element() {
            out.push_str(&text_content(child));
        }
    }
    out
}

/// Serialize an element (open tag, attributes, content, close tag) back to
/// XML text, recursively. Attribute values are escaped with the ported
/// `encode_for_xml` (`utils.h`); text content uses [`xml_text_escape`].
/// Does *not* include the element's own trailing tail text - callers walk
/// `node.children()` themselves so the following text-node sibling (the
/// "tail", in lxml terms) is picked up by the caller's own loop, exactly
/// mirroring lxml's default `with_tail=True` `tostring()` behaviour without
/// double-counting it here.
fn serialize_element_xml(node: Node) -> String {
    let mut s = String::new();
    s.push('<');
    s.push_str(node.tag_name().name());
    for attr in node.attributes() {
        s.push(' ');
        s.push_str(attr.name());
        s.push_str("=\"");
        s.push_str(&encode_for_xml(attr.value()));
        s.push('"');
    }
    let mut inner = String::new();
    for child in node.children() {
        if child.is_text() {
            inner.push_str(&xml_text_escape(child.text().unwrap_or("")));
        } else if child.is_element() {
            inner.push_str(&serialize_element_xml(child));
        }
    }
    if inner.is_empty() {
        s.push_str("/>");
    } else {
        s.push('>');
        s.push_str(&inner);
        s.push_str("</");
        s.push_str(node.tag_name().name());
        s.push('>');
    }
    s
}

/// Build the `raw` HTML-ish fragment for a `<text>`/`<paragraph>` element:
/// `escape(text.text)` followed by `tostring(child)` for each child element
/// (each of which naturally carries its own tail, per the loop structure
/// described in [`serialize_element_xml`]). Port of reflow.py lines 204-208.
fn build_raw(node: Node) -> String {
    let mut raw = String::new();
    let mut seen_element = false;
    for child in node.children() {
        if child.is_text() {
            let content = child.text().unwrap_or("");
            if seen_element {
                raw.push_str(&xml_text_escape(content));
            } else {
                raw.push_str(&html_escape_py(content));
            }
        } else if child.is_element() {
            seen_element = true;
            raw.push_str(&serialize_element_xml(child));
        }
    }
    raw
}

// ==========================================================================
// Font (reflow.py lines 109-116)
// ==========================================================================

#[derive(Debug, Clone)]
pub struct Font {
    pub id: String,
    pub size: f64,
    pub size_em: f64,
    pub color: Option<String>,
    pub family: Option<String>,
}

impl Font {
    /// Port of `Font.__init__` (reflow.py lines 111-116), parsing a
    /// `<fontspec id="..." size="..." color="..." family="..."/>` element.
    pub fn from_fontspec(node: &Node) -> Result<Font, ReflowError> {
        let id = node
            .attribute("id")
            .ok_or_else(|| ReflowError::MissingAttribute {
                tag: "fontspec".to_string(),
                attr: "id".to_string(),
            })?
            .to_string();
        let size = attr_f64(node, "fontspec", "size")?;
        Ok(Font {
            id,
            size,
            size_em: 0.0,
            color: node.attribute("color").map(str::to_string),
            family: node.attribute("family").map(str::to_string),
        })
    }
}

// ==========================================================================
// DocStats (reflow.py lines 132-137, plus fields lazily added elsewhere)
// ==========================================================================

#[derive(Debug, Clone, Default)]
pub struct DocStats {
    pub top: f64,
    pub bottom: f64,
    pub left_min_odd: f64,
    pub left_max_odd: f64,
    pub left_min_even: f64,
    pub left_max_even: f64,
    pub right: f64,
    pub line_space: f64,
    pub para_space: f64,
    pub indent_min_odd: f64,
    pub indent_max_odd: f64,
    pub indent_min_even: f64,
    pub indent_max_even: f64,
    pub font_size: f64,
    pub margin_px: f64,
}

// ==========================================================================
// Image (reflow.py lines 140-159)
// ==========================================================================

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Align {
    #[default]
    Left,
    Right,
    Center,
}

#[derive(Debug, Clone)]
pub struct Image {
    pub id: u64,
    pub top: f64,
    pub left: f64,
    pub width: f64,
    pub height: f64,
    pub bottom: f64,
    pub right: f64,
    pub src: String,
    pub align: Align,
    // Extra bookkeeping used only by the Column/Region machinery, see the
    // module doc comment.
    pub indent_fraction: f64,
    pub width_fraction: f64,
    pub top_gap_ratio: Option<f64>,
}

impl Image {
    /// Port of `Image.__init__` (reflow.py lines 142-152).
    pub fn from_node(node: &Node, idc: &mut IdGen) -> Result<Image, ReflowError> {
        let top = attr_f64(node, "image", "top")?;
        let left = attr_f64(node, "image", "left")?;
        let width = attr_f64(node, "image", "width")?;
        let height = attr_f64(node, "image", "height")?;
        let src = node
            .attribute("src")
            .ok_or_else(|| ReflowError::MissingAttribute {
                tag: "image".to_string(),
                attr: "src".to_string(),
            })?
            .to_string();
        Ok(Image {
            id: idc.next_id(),
            top,
            left,
            width,
            height,
            bottom: top + height,
            right: left + width,
            src,
            align: Align::Left,
            indent_fraction: 0.0,
            width_fraction: 0.0,
            top_gap_ratio: None,
        })
    }

    /// Port of `Image.to_html` (reflow.py lines 154-155).
    pub fn to_html(&self) -> String {
        format!(
            r#"<img src="{}" alt="" width="{}px" height="{}px"/>"#,
            self.src, self.width as i64, self.height as i64
        )
    }
}

// ==========================================================================
// Text (reflow.py lines 162-390)
// ==========================================================================

#[derive(Debug, Clone)]
pub struct Text {
    pub id: u64,
    pub top: f64,
    pub left: f64,
    pub width: f64,
    pub height: f64,
    pub bottom: f64,
    pub right: f64,
    pub tag: String,
    pub indented: i64,
    pub margin_left: i64,
    pub margin_right: i64,
    /// Position of the last line joined into this paragraph.
    pub last_left: f64,
    pub last_right: f64,
    /// Length of this line if it has been merged into a paragraph.
    pub final_width: f64,
    pub align: Align,
    pub blank_line_before: bool,
    pub blank_line_after: bool,
    pub font_id: String,
    pub font_size: f64,
    pub font_size_em: f64,
    pub color: Option<String>,
    pub font_family: Option<String>,
    pub text_as_string: String,
    pub raw: String,
    pub average_character_width: f64,
    // Extra bookkeeping used only by the Column/Region machinery.
    pub indent_fraction: f64,
    pub width_fraction: f64,
    pub top_gap_ratio: Option<f64>,
}

impl PartialEq for Text {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

static SPACES_RE: OnceLock<Regex> = OnceLock::new();
static ITALIC_EMPTY_RE: OnceLock<Regex> = OnceLock::new();
static BOLD_EMPTY_RE: OnceLock<Regex> = OnceLock::new();

impl Text {
    /// Port of `Text.__init__` (reflow.py lines 164-209).
    pub fn from_node(
        node: Node,
        font_map: &IndexMap<String, Font>,
        idc: &mut IdGen,
    ) -> Result<Text, ReflowError> {
        let id = idc.next_id();
        let top = py_round(attr_f64(&node, "text", "top")?);
        let left = py_round(attr_f64(&node, "text", "left")?);
        let width = py_round(attr_f64(&node, "text", "width")?);
        let height = py_round(attr_f64(&node, "text", "height")?);
        let bottom = top + height;
        let right = left + width;

        let (font_id, font_size, font_size_em, color, font_family) = if !font_map.is_empty() {
            let fid = node
                .attribute("font")
                .ok_or_else(|| ReflowError::MissingAttribute {
                    tag: "text".to_string(),
                    attr: "font".to_string(),
                })?;
            let font = font_map
                .get(fid)
                .ok_or_else(|| ReflowError::UnknownFont(fid.to_string()))?;
            (
                font.id.clone(),
                font.size,
                font.size_em,
                font.color.clone(),
                font.family.clone(),
            )
        } else {
            (String::new(), 0.0, 0.0, None, None)
        };

        let text_as_string = text_content(node);
        let raw = build_raw(node);

        let mut t = Text {
            id,
            top,
            left,
            width,
            height,
            bottom,
            right,
            tag: "p".to_string(),
            indented: 0,
            margin_left: 0,
            margin_right: 0,
            last_left: left,
            last_right: right,
            final_width: width,
            align: Align::Left,
            blank_line_before: false,
            blank_line_after: false,
            font_id,
            font_size,
            font_size_em,
            color,
            font_family,
            text_as_string,
            raw,
            average_character_width: 0.1,
            indent_fraction: 0.0,
            width_fraction: 0.0,
            top_gap_ratio: None,
        };
        t.set_av_char_width();
        Ok(t)
    }

    /// There is nothing in this Text. Port of `Text.is_empty` (line 211-214).
    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }

    /// There are only spaces in this Text. Port of `Text.is_spaces`
    /// (reflow.py lines 216-223).
    pub fn is_spaces(&self) -> bool {
        if self.raw.is_empty() {
            return false;
        }
        let spaces_re = SPACES_RE.get_or_init(|| Regex::new(r"^\s+$").expect("static regex"));
        let italic_re = ITALIC_EMPTY_RE
            .get_or_init(|| Regex::new(r"^\s*<i>\s*</i>\s*$").expect("static regex"));
        let bold_re =
            BOLD_EMPTY_RE.get_or_init(|| Regex::new(r"^\s*<b>\s*</b>\s*$").expect("static regex"));
        spaces_re.is_match(&self.raw)
            || italic_re.is_match(&self.raw)
            || bold_re.is_match(&self.raw)
    }

    /// Port of `Text.set_av_char_width` (reflow.py lines 225-226).
    pub fn set_av_char_width(&mut self) {
        let len = self.text_as_string.chars().count().max(1) as f64;
        self.average_character_width = (self.width / len).max(0.1);
    }

    /// Port of `Text.to_html` (reflow.py lines 384-385).
    pub fn to_html(&self) -> String {
        self.raw.clone()
    }

    /// Port of `Text.coalesce` (reflow.py lines 228-381): merge `other`
    /// into `self`, joining fragments/lines into a single logical text run.
    /// `other` is cloned first because the Python original mutates the
    /// passed-in object's `.raw` in place (see the href-merge block below)
    /// before folding it into `self` - an owned local copy sidesteps a
    /// double-mutable-borrow that isn't expressible while keeping the
    /// signature ergonomic (`self` and `other` both live in the same
    /// `Vec<Text>` in every call site).
    pub fn coalesce(
        &mut self,
        other: &Text,
        _page_number: i64,
        _left_margin: f64,
        right_margin: f64,
    ) {
        let mut other = other.clone();
        let mut has_float = String::new();

        let has_gap: i64;
        if self.top <= other.top
            && self.bottom >= other.bottom
            && (other.left - self.right).abs() < 2.0
        {
            has_gap = 0;
        } else if other.left < self.right {
            has_gap = 1;
        } else {
            has_gap =
                ((other.left - self.right) / self.average_character_width + 0.5).round() as i64;
        }

        if other.left >= self.right {
            // Same line. Allow for super/subscript: use the taller side's
            // top/bottom.
            if self.height >= other.height {
                self.top = self.top.min(other.top);
                self.bottom = self.bottom.max(other.bottom);
            } else {
                self.top = other.top;
                self.bottom = other.bottom;
            }
        } else {
            self.top = self.top.min(other.top);
            self.bottom = self.bottom.max(other.bottom);
        }

        self.left = self.left.min(other.left);
        self.right = self.right.max(other.right);
        self.width += other.width;
        self.final_width = other.left + other.width;
        self.height = self.bottom - self.top;

        // NOTE: the Python `if self.font_size_em == other.font_size_em and
        // False and ...:` branch is unreachable (guarded by a literal
        // `False`) and is omitted here; only the `elif` survives upstream.
        if self.font_size_em != other.font_size_em && self.font_size_em != 1.0 {
            if !self.raw.starts_with("<span") {
                self.raw = format!(
                    r#"<span style="font-size:{}em">{}</span>"#,
                    format_py_float(self.font_size_em),
                    self.raw
                );
            } else if self.text_as_string.chars().count() <= 2
                && self.font_size_em >= other.font_size_em * 2.0
            {
                static SPLIT_RE: OnceLock<Regex> = OnceLock::new();
                let re =
                    SPLIT_RE.get_or_init(|| Regex::new(r#"^(.+em">)(.+)$"#).expect("static regex"));
                if let Some(caps) = re.captures(&self.raw) {
                    let head = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    let tail = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                    self.raw = format!(
                        r#"{head}<span style="float:left"><span style="line-height:0.5">{tail}</span></span>"#
                    );
                }
                // else: no-op. The Python original would raise
                // AttributeError here (`m_self.group(1)` on a `None`
                // match) if `self.raw` starts with `<span` but doesn't
                // match `.+em">.+`; per docs/FAULT_TOLERANCE.md this port
                // degrades gracefully instead of panicking.
            }
        }

        self.font_size = self.font_size.max(other.font_size);
        self.font_size_em = self.font_size_em.max(other.font_size_em);
        self.font_id = other.font_id.clone();
        self.color = other.color.clone();
        self.font_family = other.font_family.clone();

        let mut has_gap = has_gap;
        if has_gap > 0 {
            if has_gap < 3 {
                let self_ends_gapish = self.text_as_string.ends_with(' ')
                    || self.text_as_string.ends_with('-')
                    || other.text_as_string.starts_with(' ')
                    || other.text_as_string.starts_with('-');
                has_gap = if !self_ends_gapish { 1 } else { 0 };
            } else if !self.text_as_string.contains("   ")
                && other.right > right_margin - right_margin * RIGHT_FLOAT_FACTOR
            {
                has_float = r#"<span style="float:right">"#.to_string();
                has_gap = 1;
            }

            static OLD_FLOAT_RE: OnceLock<Regex> = OnceLock::new();
            let old_float_re = OLD_FLOAT_RE.get_or_init(|| {
                Regex::new(r#"^(.*)(<span style="float:right">.*)</span>\s*$"#)
                    .expect("static regex")
            });
            if let Some(caps) = old_float_re.captures(&self.raw) {
                let r1 = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let r2 = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                self.raw = format!("{r1}{r2}");
                has_float = " ".to_string();
            }

            while has_gap > 0 {
                self.text_as_string.push(' ');
                self.raw.push(' ');
                self.width += self.average_character_width;
                has_gap -= 1;
            }
        }

        self.text_as_string.push_str(&other.text_as_string);

        // Try to merge <a href=...> where both sides point at the same
        // place. Port of reflow.py lines 349-369.
        static SELF_HREF_RE: OnceLock<Regex> = OnceLock::new();
        static OTHER_HREF_RE: OnceLock<Regex> = OnceLock::new();
        let self_href_re = SELF_HREF_RE.get_or_init(|| {
            Regex::new(r#"^([^<]*)(<span[^>]*>)*(<a href[^>]+>)(.*)</a>(</span>)*(\s*)$"#)
                .expect("static regex")
        });
        let other_href_re = OTHER_HREF_RE.get_or_init(|| {
            Regex::new(r#"^([^<]*)(<span[^>]*>)*(<a href[^>]+>)(.*)(</a>)(</span>)*(.*)$"#)
                .expect("static regex")
        });
        if let Some(m) = self_href_re.captures(&self.raw) {
            if let Some(o) = other_href_re.captures(&other.raw) {
                let m3 = m.get(3).map(|x| x.as_str()).unwrap_or("");
                let o3 = o.get(3).map(|x| x.as_str()).unwrap_or("");
                if m3 == o3 {
                    let m2 = m.get(2).map(|x| x.as_str()).unwrap_or("");
                    let m5 = m.get(5).map(|x| x.as_str()).unwrap_or("");
                    let o2 = o.get(2).map(|x| x.as_str()).unwrap_or("");
                    let o6 = o.get(6).map(|x| x.as_str()).unwrap_or("");
                    let o4 = o.get(4).map(|x| x.as_str()).unwrap_or("");
                    let o5 = o.get(5).map(|x| x.as_str()).unwrap_or("");
                    let o7 = o.get(7).map(|x| x.as_str()).unwrap_or("");
                    let o1 = o.get(1).map(|x| x.as_str()).unwrap_or("");
                    other.raw = format!("{o1}{o2}{o4}{o6}{o5}{o7}");
                    let m1 = m.get(1).map(|x| x.as_str()).unwrap_or("");
                    let m4 = m.get(4).map(|x| x.as_str()).unwrap_or("");
                    let m6 = m.get(6).map(|x| x.as_str()).unwrap_or("");
                    self.raw = format!("{m1}{m3}{m2}{m4}{m5}{m6}");
                }
            }
        }

        if !has_float.is_empty() {
            self.raw.push_str(&has_float);
        }
        self.raw.push_str(&other.raw);
        if !has_float.is_empty() {
            self.raw.push_str("</span>");
        }
        self.set_av_char_width();
    }
}

/// Format a float the way Python's `str()`/f-string `{!s}` would for the
/// values that flow through here (font-size-em multipliers like `1.0`,
/// `0.7`, `1.5`): trim a trailing `.0` only when the value truly is a
/// whole number is *not* what Python does (`str(1.0) == '1.0'`), so this
/// simply defers to Rust's `Display` for `f64`, which already matches
/// Python's `repr`/`str` for these small decimal values in practice.
fn format_py_float(v: f64) -> String {
    format!("{v}")
}

// ==========================================================================
// Paragraph (reflow.py lines 393-428)
//
// Dead code upstream: defined but never instantiated anywhere in
// reflow.py (confirmed by grep - the only occurrence of `Paragraph(` is
// the class statement itself). Its Python `__init__` even calls
// `Text.__init__(self)` with no further arguments, which would raise a
// `TypeError` if `Paragraph(...)` were ever actually called - a second
// sign this class is vestigial. Ported here as a real, working
// (non-buggy) near-duplicate of `Text::from_node` for API completeness,
// per this port's brief to cover the full named class list faithfully.
// ==========================================================================

#[derive(Debug, Clone)]
pub struct Paragraph {
    pub id: u64,
    pub top: f64,
    pub left: f64,
    pub width: f64,
    pub height: f64,
    pub bottom: f64,
    pub right: f64,
    pub font_id: String,
    pub font_size: f64,
    pub color: Option<String>,
    pub font_family: Option<String>,
    pub text_as_string: String,
    pub raw: String,
    pub average_character_width: f64,
}

impl Paragraph {
    /// Port of `Paragraph.__init__` (reflow.py lines 395-420).
    pub fn from_node(
        node: Node,
        font_map: &IndexMap<String, Font>,
        idc: &mut IdGen,
    ) -> Result<Paragraph, ReflowError> {
        let id = idc.next_id();
        let top = attr_f64(&node, "text", "top")?;
        let left = attr_f64(&node, "text", "left")?;
        let width = attr_f64(&node, "text", "width")?;
        let height = attr_f64(&node, "text", "height")?;

        let (font_id, font_size, color, font_family) = if !font_map.is_empty() {
            let fid = node
                .attribute("font")
                .ok_or_else(|| ReflowError::MissingAttribute {
                    tag: "text".to_string(),
                    attr: "font".to_string(),
                })?;
            let font = font_map
                .get(fid)
                .ok_or_else(|| ReflowError::UnknownFont(fid.to_string()))?;
            (
                font.id.clone(),
                font.size,
                font.color.clone(),
                font.family.clone(),
            )
        } else {
            (String::new(), 0.0, None, None)
        };

        let text_as_string = text_content(node);
        let raw = build_raw(node);
        let mut p = Paragraph {
            id,
            top,
            left,
            width,
            height,
            bottom: top + height,
            right: left + width,
            font_id,
            font_size,
            color,
            font_family,
            text_as_string,
            raw,
            average_character_width: 0.1,
        };
        p.set_av_char_width();
        Ok(p)
    }

    fn set_av_char_width(&mut self) {
        let len = self.text_as_string.chars().count().max(1) as f64;
        self.average_character_width = (self.width / len).max(0.1);
    }

    /// Port of `Paragraph.to_html` (reflow.py lines 422-423).
    pub fn to_html(&self) -> String {
        self.raw.clone()
    }
}

// ==========================================================================
// FontSizeStats (reflow.py lines 431-440)
// ==========================================================================

/// Port of `FontSizeStats(dict)`: turns `{size: char_count}` into
/// `{size: fraction_of_total_chars}`, remembering the most common size.
/// An `IndexMap` (not `HashMap`) is used to preserve insertion order,
/// matching Python 3.7+ dict iteration order - which matters here because
/// the tie-break (`chars >= self.chars_at_most_common_size`) picks the
/// *last* inserted key on a tie.
#[derive(Debug, Clone, Default)]
pub struct FontSizeStats {
    pub ratios: IndexMap<FKey, f64>,
    pub most_common_size: f64,
    pub chars_at_most_common_size: i64,
}

impl FontSizeStats {
    /// Port of `FontSizeStats.__init__` (reflow.py lines 433-440).
    pub fn new(stats: &IndexMap<FKey, i64>) -> FontSizeStats {
        let total: f64 = stats.values().sum::<i64>() as f64;
        let mut most_common_size = -1.0;
        let mut chars_at_most_common_size = 0i64;
        let mut ratios = IndexMap::new();
        for (&sz, &chars) in stats.iter() {
            if chars >= chars_at_most_common_size {
                most_common_size = sz.value();
                chars_at_most_common_size = chars;
            }
            let ratio = if total > 0.0 {
                chars as f64 / total
            } else {
                0.0
            };
            ratios.insert(sz, ratio);
        }
        FontSizeStats {
            ratios,
            most_common_size,
            chars_at_most_common_size,
        }
    }
}

// ==========================================================================
// LineElem: a Text or an Image, used by the Column/Box/Region machinery
// below (which operates on whichever kind of element occupies a "row" on
// the page). See the module doc comment for why this is faithfully ported
// despite being unreachable from `PdfDocument`'s real pipeline.
// ==========================================================================

#[derive(Debug, Clone)]
pub enum LineElem {
    Text(Text),
    Image(Image),
}

impl LineElem {
    pub fn elem_id(&self) -> u64 {
        match self {
            LineElem::Text(t) => t.id,
            LineElem::Image(i) => i.id,
        }
    }
    pub fn left(&self) -> f64 {
        match self {
            LineElem::Text(t) => t.left,
            LineElem::Image(i) => i.left,
        }
    }
    pub fn right(&self) -> f64 {
        match self {
            LineElem::Text(t) => t.right,
            LineElem::Image(i) => i.right,
        }
    }
    pub fn top(&self) -> f64 {
        match self {
            LineElem::Text(t) => t.top,
            LineElem::Image(i) => i.top,
        }
    }
    pub fn bottom(&self) -> f64 {
        match self {
            LineElem::Text(t) => t.bottom,
            LineElem::Image(i) => i.bottom,
        }
    }
    pub fn width(&self) -> f64 {
        match self {
            LineElem::Text(t) => t.width,
            LineElem::Image(i) => i.width,
        }
    }
    pub fn set_indent_fraction(&mut self, v: f64) {
        match self {
            LineElem::Text(t) => t.indent_fraction = v,
            LineElem::Image(i) => i.indent_fraction = v,
        }
    }
    pub fn set_width_fraction(&mut self, v: f64) {
        match self {
            LineElem::Text(t) => t.width_fraction = v,
            LineElem::Image(i) => i.width_fraction = v,
        }
    }
    pub fn set_top_gap_ratio(&mut self, v: Option<f64>) {
        match self {
            LineElem::Text(t) => t.top_gap_ratio = v,
            LineElem::Image(i) => i.top_gap_ratio = v,
        }
    }
    pub fn top_gap_ratio(&self) -> Option<f64> {
        match self {
            LineElem::Text(t) => t.top_gap_ratio,
            LineElem::Image(i) => i.top_gap_ratio,
        }
    }
    pub fn indent_fraction(&self) -> f64 {
        match self {
            LineElem::Text(t) => t.indent_fraction,
            LineElem::Image(i) => i.indent_fraction,
        }
    }
    pub fn is_image(&self) -> bool {
        matches!(self, LineElem::Image(_))
    }
    pub fn to_html(&self) -> String {
        match self {
            LineElem::Text(t) => t.to_html(),
            LineElem::Image(i) => i.to_html(),
        }
    }
}

impl PartialEq for LineElem {
    fn eq(&self, other: &Self) -> bool {
        self.elem_id() == other.elem_id()
    }
}

// ==========================================================================
// Interval (reflow.py lines 443-466)
// ==========================================================================

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Interval {
    pub left: f64,
    pub right: f64,
    pub width: f64,
}

impl Interval {
    /// Port of `Interval.__init__` (reflow.py lines 445-447).
    pub fn new(left: f64, right: f64) -> Interval {
        Interval {
            left,
            right,
            width: right - left,
        }
    }

    /// Port of `Interval.intersection` (reflow.py lines 449-452).
    pub fn intersection(&self, other: &Interval) -> Interval {
        let left = self.left.max(other.left);
        let right = self.right.min(other.right);
        Interval::new(left, right)
    }

    /// Port of `Interval.centered_in` (reflow.py lines 454-457).
    pub fn centered_in(&self, parent: &Interval) -> bool {
        let left = (self.left - parent.left).abs();
        let right = (self.right - parent.right).abs();
        (left - right).abs() < 3.0
    }

    /// Port of `Interval.__nonzero__` (reflow.py lines 459-460): whether
    /// the interval has positive width.
    pub fn is_nonzero(&self) -> bool {
        self.width > 0.0
    }
}

// ==========================================================================
// Column (reflow.py lines 469-537)
// ==========================================================================

#[derive(Debug, Clone, Default)]
pub struct Column {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
    pub width: f64,
    pub height: f64,
    pub elements: Vec<LineElem>,
    pub average_line_separation: f64,
}

impl Column {
    /// Elements bulge in/out by at most this fraction of the column width
    /// and still count as belonging to the column.
    pub const HFUZZ: f64 = 0.2;

    pub fn new() -> Column {
        Column::default()
    }

    /// Port of `Column.add` (reflow.py lines 481-485).
    pub fn add(&mut self, elem: LineElem) {
        if self.elements.contains(&elem) {
            return;
        }
        self.elements.push(elem);
        self.post_add();
    }

    /// Port of `Column.prepend` (reflow.py lines 487-491).
    pub fn prepend(&mut self, elem: LineElem) {
        if self.elements.contains(&elem) {
            return;
        }
        self.elements.insert(0, elem);
        self.post_add();
    }

    /// Port of `Column._post_add` (reflow.py lines 493-501).
    fn post_add(&mut self) {
        self.elements.sort_by(|a, b| {
            a.bottom()
                .partial_cmp(&b.bottom())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        self.top = self.elements[0].top();
        self.bottom = self.elements[self.elements.len() - 1].bottom();
        self.left = f64::MAX;
        self.right = 0.0;
        for x in &self.elements {
            self.left = self.left.min(x.left());
            self.right = self.right.max(x.right());
        }
        self.width = self.right - self.left;
        self.height = self.bottom - self.top;
    }

    pub fn len(&self) -> usize {
        self.elements.len()
    }

    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, LineElem> {
        self.elements.iter()
    }

    /// Port of `Column.contains` (reflow.py lines 509-511).
    pub fn contains(&self, elem: &LineElem) -> bool {
        elem.left() > self.left - Self::HFUZZ * self.width
            && elem.right() < self.right + Self::HFUZZ * self.width
    }

    /// Port of `Column.collect_stats` (reflow.py lines 513-526).
    pub fn collect_stats(&mut self) {
        if self.elements.len() > 1 {
            let gaps: Vec<f64> = (0..self.elements.len() - 1)
                .map(|i| self.elements[i + 1].top() - self.elements[i].bottom())
                .collect();
            self.average_line_separation = gaps.iter().sum::<f64>() / gaps.len() as f64;
        }
        let width = self.width;
        let left = self.left;
        let avg_sep = self.average_line_separation;
        let prev_bottoms: Vec<f64> = self.elements.iter().map(|e| e.bottom()).collect();
        for (i, elem) in self.elements.iter_mut().enumerate() {
            let left_margin = elem.left() - left;
            elem.set_indent_fraction(left_margin / width);
            elem.set_width_fraction(elem.width() / width);
            if i == 0 || avg_sep == 0.0 {
                elem.set_top_gap_ratio(None);
            } else {
                elem.set_top_gap_ratio(Some((prev_bottoms[i - 1] - elem.top()) / avg_sep));
            }
        }
    }

    /// Port of `Column.previous_element` (reflow.py lines 528-531).
    pub fn previous_element(&self, idx: usize) -> Option<&LineElem> {
        if idx == 0 {
            None
        } else {
            self.elements.get(idx - 1)
        }
    }
}

// ==========================================================================
// HtmlBox / ImageBox (reflow.py lines 539-572: `Box`/`ImageBox`)
// ==========================================================================

#[derive(Debug, Clone)]
pub enum BoxItem {
    Elem(Box<LineElem>),
    PageMarker(i64),
}

/// Port of `Box(list)` (reflow.py lines 539-552). Named `HtmlBox` because
/// `Box` collides with `std::boxed::Box`.
#[derive(Debug, Clone)]
pub struct HtmlBox {
    pub tag: String,
    pub items: Vec<BoxItem>,
}

impl Default for HtmlBox {
    fn default() -> Self {
        HtmlBox {
            tag: "p".to_string(),
            items: Vec::new(),
        }
    }
}

impl HtmlBox {
    pub fn new(tag: impl Into<String>) -> HtmlBox {
        HtmlBox {
            tag: tag.into(),
            items: Vec::new(),
        }
    }

    pub fn push(&mut self, item: BoxItem) {
        self.items.push(item);
    }

    pub fn insert_front(&mut self, item: BoxItem) {
        self.items.insert(0, item);
    }

    /// Port of `Box.to_html` (reflow.py lines 544-552).
    pub fn to_html(&self) -> Vec<String> {
        let mut ans = vec![format!("<{}>", self.tag)];
        for item in &self.items {
            match item {
                BoxItem::PageMarker(n) => ans.push(format!(r#"<a name="page_{n}"/>"#)),
                BoxItem::Elem(e) => ans.push(format!("{} ", e.to_html())),
            }
        }
        ans.push(format!("</{}>", self.tag));
        ans
    }
}

/// Port of `ImageBox(Box)` (reflow.py lines 555-572).
#[derive(Debug, Clone)]
pub struct ImageBox {
    pub img: Image,
    pub inner: HtmlBox,
}

impl ImageBox {
    pub fn new(img: Image) -> ImageBox {
        ImageBox {
            img,
            inner: HtmlBox::default(),
        }
    }

    /// Port of `ImageBox.to_html` (reflow.py lines 561-572).
    pub fn to_html(&self) -> Vec<String> {
        let mut ans = vec![r#"<div style="text-align:center">"#.to_string()];
        ans.push(self.img.to_html());
        if !self.inner.items.is_empty() {
            ans.push("<br/>".to_string());
            for item in &self.inner.items {
                match item {
                    BoxItem::PageMarker(n) => ans.push(format!(r#"<a name="page_{n}"/>"#)),
                    BoxItem::Elem(e) => ans.push(format!("{} ", e.to_html())),
                }
            }
        }
        ans.push("</div>".to_string());
        ans
    }
}

/// Either kind of box `Region.boxes`/`PDFDocument.elements` can hold in the
/// original (`Box` and `ImageBox` are both plain Python lists there, one
/// subclassing the other, so a single list can mix both freely).
#[derive(Debug, Clone)]
pub enum RegionBox {
    Html(HtmlBox),
    Image(ImageBox),
}

impl RegionBox {
    pub fn to_html(&self) -> Vec<String> {
        match self {
            RegionBox::Html(b) => b.to_html(),
            RegionBox::Image(b) => b.to_html(),
        }
    }

    pub fn push(&mut self, item: BoxItem) {
        match self {
            RegionBox::Html(b) => b.push(item),
            RegionBox::Image(b) => b.inner.push(item),
        }
    }

    pub fn insert_front(&mut self, item: BoxItem) {
        match self {
            RegionBox::Html(b) => b.insert_front(item),
            RegionBox::Image(b) => b.inner.insert_front(item),
        }
    }
}

// ==========================================================================
// Region (reflow.py lines 575-728)
// ==========================================================================

#[derive(Debug, Clone, Default)]
pub struct Region {
    pub columns: Vec<Column>,
    pub top: f64,
    pub bottom: f64,
    pub left: f64,
    pub right: f64,
    pub width: f64,
    pub height: f64,
    pub average_line_separation: f64,
    /// Populated by `linearize()`.
    pub boxes: Vec<RegionBox>,
}

impl Region {
    pub fn new() -> Region {
        Region::default()
    }

    /// Port of `Region.add` (reflow.py lines 582-589).
    pub fn add(&mut self, columns: Vec<Column>) {
        if self.columns.is_empty() {
            let mut cols = columns;
            cols.sort_by(|a, b| {
                a.left
                    .partial_cmp(&b.left)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            self.columns = cols;
        } else {
            for (i, col) in columns.into_iter().enumerate() {
                if let Some(dest) = self.columns.get_mut(i) {
                    for elem in col.elements {
                        dest.add(elem);
                    }
                }
            }
        }
    }

    /// Port of `Region.contains` (reflow.py lines 591-605). Note: upstream
    /// leaves unbalanced-column handling as a `TODO`.
    pub fn contains(&self, columns: &[Column]) -> bool {
        if self.columns.is_empty() {
            return true;
        }
        if columns.len() != self.columns.len() {
            return false;
        }
        for (c1, c2) in self.columns.iter().zip(columns.iter()) {
            let x1 = Interval::new(c1.left, c1.right);
            let x2 = Interval::new(c2.left, c2.right);
            let intersection = x1.intersection(&x2);
            let base = x1.width.min(x2.width);
            if base == 0.0 || intersection.width / base < 0.6 {
                return false;
            }
        }
        true
    }

    /// Port of `Region.is_empty` (reflow.py lines 607-609).
    pub fn is_empty_region(&self) -> bool {
        self.columns.is_empty()
    }

    /// Port of `Region.line_count` (reflow.py lines 611-616).
    pub fn line_count(&self) -> usize {
        self.columns.iter().map(|c| c.len()).max().unwrap_or(0)
    }

    /// Port of `Region.is_small` (reflow.py lines 618-620).
    pub fn is_small(&self) -> bool {
        self.line_count() < 3
    }

    /// Port of `Region.absorb` (reflow.py lines 622-645).
    pub fn absorb(&mut self, singleton: &Region, log: &mut ReflowLog, verbose: i32) {
        let cols_snapshot: Vec<(f64, f64)> =
            self.columns.iter().map(|c| (c.left, c.right)).collect();
        for c in &singleton.columns {
            for elem in c.iter() {
                let mut mc_idx = None;
                let mut mw = 0.0;
                let e = Interval::new(elem.left(), elem.right());
                for (idx, &(cl, cr)) in cols_snapshot.iter().enumerate() {
                    let i = Interval::new(cl, cr);
                    let w = i.intersection(&e).width;
                    if w > mw {
                        mc_idx = Some(idx);
                        mw = w;
                    }
                }
                let idx = match mc_idx {
                    Some(idx) => idx,
                    None => {
                        log.warn(format!(
                            "No suitable column for singleton {}",
                            elem.to_html()
                        ));
                        0
                    }
                };
                if verbose > 3 {
                    log.debug(format!(
                        "Absorbing singleton {} into column {idx}",
                        elem.to_html()
                    ));
                }
                if let Some(col) = self.columns.get_mut(idx) {
                    col.add(elem.clone());
                }
            }
        }
    }

    /// Port of `Region.collect_stats` (reflow.py lines 647-650).
    pub fn collect_stats(&mut self) {
        for column in &mut self.columns {
            column.collect_stats();
        }
        let n = self.columns.len().max(1) as f64;
        self.average_line_separation = self
            .columns
            .iter()
            .map(|c| c.average_line_separation)
            .sum::<f64>()
            / n;
    }

    /// Port of `Region.absorb_regions` (reflow.py lines 655-657).
    pub fn absorb_regions(&mut self, regions: &[Region], at: &str) {
        for region in regions {
            self.absorb_region(region, at);
        }
    }

    /// Port of `Region.absorb_region` (reflow.py lines 659-691).
    pub fn absorb_region(&mut self, region: &Region, at: &str) {
        if region.columns.len() <= self.columns.len() {
            for i in 0..region.columns.len() {
                let src_elems: Vec<LineElem> = if at != "bottom" {
                    region.columns[i].elements.iter().rev().cloned().collect()
                } else {
                    region.columns[i].elements.clone()
                };
                if let Some(dest) = self.columns.get_mut(i) {
                    for elem in src_elems {
                        if at == "bottom" {
                            dest.add(elem);
                        } else {
                            dest.prepend(elem);
                        }
                    }
                }
            }
        } else {
            let mut col_map: HashMap<usize, usize> = HashMap::new();
            for (i, col) in region.columns.iter().enumerate() {
                let mut max_overlap = 0.0;
                let mut max_overlap_index = 0;
                for (j, dcol) in self.columns.iter().enumerate() {
                    let sint = Interval::new(col.left, col.right);
                    let dint = Interval::new(dcol.left, dcol.right);
                    let width = sint.intersection(&dint).width;
                    if width > max_overlap {
                        max_overlap = width;
                        max_overlap_index = j;
                    }
                }
                col_map.insert(i, max_overlap_index);
            }
            let lines = region.columns.iter().map(|c| c.len()).max().unwrap_or(0);
            let order: Vec<usize> = if at == "bottom" {
                (0..lines).collect()
            } else {
                (0..lines).rev().collect()
            };
            for i in order {
                for (j, src) in region.columns.iter().enumerate() {
                    if i < src.len() {
                        if let Some(&dest_idx) = col_map.get(&j) {
                            if let Some(dest) = self.columns.get_mut(dest_idx) {
                                let elem = src.elements[i].clone();
                                if at == "bottom" {
                                    dest.add(elem);
                                } else {
                                    dest.prepend(elem);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Port of `Region.linearize` (reflow.py lines 700-727): split the
    /// region's linear stream of elements into `HtmlBox`es, breaking on
    /// images and on paragraph-gap/indent heuristics.
    pub fn linearize(&mut self) {
        let mut elements: Vec<LineElem> = Vec::new();
        for col in &self.columns {
            elements.extend(col.elements.iter().cloned());
        }
        let mut boxes: Vec<RegionBox> = vec![RegionBox::Html(HtmlBox::default())];
        let mut i = 0usize;
        while i < elements.len() {
            let elem = elements[i].clone();
            if let LineElem::Image(img) = &elem {
                let mut ibox = ImageBox::new(img.clone());
                let image_interval = Interval::new(img.left, img.right);
                let mut j = i + 1;
                while j < elements.len() {
                    let t = &elements[j];
                    if t.is_image() {
                        break;
                    }
                    let ti = Interval::new(t.left(), t.right());
                    if !ti.centered_in(&image_interval) {
                        break;
                    }
                    ibox.inner.push(BoxItem::Elem(Box::new(t.clone())));
                    j += 1;
                }
                boxes.push(RegionBox::Image(ibox));
                boxes.push(RegionBox::Html(HtmlBox::default()));
                i = j;
                continue;
            }
            let mut is_indented = false;
            if i + 1 < elements.len() {
                let indent_diff = elem.indent_fraction() - elements[i + 1].indent_fraction();
                if indent_diff > 0.05 {
                    is_indented = true;
                }
            }
            if elem.top_gap_ratio().unwrap_or(0.0) > 1.2 || is_indented {
                boxes.push(RegionBox::Html(HtmlBox::default()));
            }
            boxes
                .last_mut()
                .expect("boxes always has >=1 element")
                .push(BoxItem::Elem(Box::new(elem)));
            i += 1;
        }
        self.boxes = boxes;
    }
}

// ==========================================================================
// XML tree-walking helper (stand-in for `Element.xpath('descendant::tag')`,
// which lxml provides and roxmltree does not).
// ==========================================================================

fn descendants_with_tag<'a, 'input>(node: Node<'a, 'input>, tag: &str) -> Vec<Node<'a, 'input>> {
    node.descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == tag)
        .collect()
}

// ==========================================================================
// Page (reflow.py lines 730-1419)
// ==========================================================================

#[derive(Debug, Clone, Default)]
pub struct Page {
    pub number: i64,
    pub odd_even: i64,
    pub top: f64,
    pub left: f64,
    pub width: f64,
    pub height: f64,
    pub id: String,
    pub page_break_after: bool,
    pub texts: Vec<Text>,
    pub imgs: Vec<Image>,
    pub left_margin: f64,
    pub right_margin: f64,
    pub id_used: bool,
    pub textwidth: f64,
    pub font_size_stats: FontSizeStats,
    pub document_font_stats: Option<FontSizeStats>,
    pub average_text_height: f64,
    pub stats_left_min: f64,
    pub stats_left_max: f64,
    pub stats_indent_min: f64,
    pub stats_indent_max: f64,
    pub stats_right: f64,
    pub stats_margin_px: f64,
    pub contents: bool,
    /// Populated only by the unreachable-upstream Region machinery; see the
    /// module doc comment.
    pub regions: Vec<Region>,
}

fn text_cmp(a: &Text, b: &Text) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if (a.top <= b.top && a.bottom >= b.bottom - BOTTOM_FACTOR)
        || (b.top <= a.top && b.bottom >= a.bottom - BOTTOM_FACTOR)
    {
        if a.left < b.left {
            Ordering::Less
        } else if a.left == b.left {
            Ordering::Equal
        } else {
            Ordering::Greater
        }
    } else if a.bottom < b.bottom {
        Ordering::Less
    } else if a.bottom == b.bottom {
        Ordering::Equal
    } else {
        Ordering::Greater
    }
}

fn regex_lead_strip() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*(<[^>]+>)?\s*(.*)$").expect("static regex"))
}

fn regex_href_pagenum() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"^(.*)(<a href)(.+)("index\.html#)(\d+)(".+)$"#).expect("static regex")
    })
}

impl Page {
    /// Port of `Page.__init__` (reflow.py lines 732-848).
    pub fn from_node(
        page: Node,
        font_map: &IndexMap<String, Font>,
        opts: &ReflowOpts,
        idc: &mut IdGen,
    ) -> Result<Page, ReflowError> {
        let number = page
            .attribute("number")
            .ok_or_else(|| ReflowError::MissingAttribute {
                tag: "page".to_string(),
                attr: "number".to_string(),
            })?
            .parse::<i64>()
            .map_err(|_| ReflowError::InvalidNumber {
                tag: "page".to_string(),
                attr: "number".to_string(),
                value: page.attribute("number").unwrap_or("").to_string(),
            })?;
        let odd_even = number % 2;
        let top = attr_f64(&page, "page", "top")?;
        let left = attr_f64(&page, "page", "left")?;
        let width = attr_f64(&page, "page", "width")?;
        let height = attr_f64(&page, "page", "height")?;
        let id = format!("page{number}");

        let mut texts: Vec<Text> = Vec::new();
        let mut left_margin = width;
        let mut right_margin = 0.0f64;

        for text_node in descendants_with_tag(page, "text") {
            let mut t = Text::from_node(text_node, font_map, idc)?;
            if t.is_spaces()
                || t.top < top
                || t.top > height
                || t.left > left + width
                || t.left < left
            {
                // Outside page boundaries, or spaces-only: discard.
                continue;
            } else if (opts.pdf_header_skip <= 0.0 || t.top >= opts.pdf_header_skip)
                && (opts.pdf_footer_skip <= 0.0 || t.top <= opts.pdf_footer_skip)
            {
                // Turn 3+ leading spaces into a text-indent.
                let chars: Vec<char> = t.text_as_string.chars().collect();
                let mut s = 0usize;
                while s < chars.len() && chars[s] == ' ' {
                    s += 1;
                }
                if s > 2 {
                    t.indented = 1;
                    let w = py_round(s as f64 * t.average_character_width / 2.0);
                    if let Some(caps) = regex_lead_strip().captures(&t.raw) {
                        let t1 = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                        let t2 = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                        t.raw = format!("{t1}{t2}");
                    }
                    t.text_as_string = chars[s..].iter().collect();
                    t.left += w;
                    t.last_left += w;
                    t.width -= w;
                    t.final_width -= w;
                }
                left_margin = left_margin.min(t.left);
                right_margin = right_margin.max(t.right);
                if let Some(caps) = regex_href_pagenum().captures(&t.raw) {
                    let g1 = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    let g2 = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                    let g3 = caps.get(3).map(|m| m.as_str()).unwrap_or("");
                    let g4 = caps.get(4).map(|m| m.as_str()).unwrap_or("");
                    let g5 = caps.get(5).map(|m| m.as_str()).unwrap_or("");
                    let g6 = caps.get(6).map(|m| m.as_str()).unwrap_or("");
                    t.raw = format!("{g1}{g2}{g3}{g4}page_{g5}{g6}");
                }
                texts.push(t);
            }
            // else: not within header/footer bounds -> discard.
        }

        let mut imgs: Vec<Image> = Vec::new();
        if !opts.no_images {
            for img_node in descendants_with_tag(page, "image") {
                imgs.push(Image::from_node(&img_node, idc)?);
            }
        }

        let textwidth = right_margin - left_margin;

        texts.sort_by(text_cmp);

        let mut font_size_stats_raw: IndexMap<FKey, i64> = IndexMap::new();
        let mut average_text_height = 0.0;
        for t in &texts {
            let key = FKey::new(t.font_size);
            *font_size_stats_raw.entry(key).or_insert(0) += t.text_as_string.chars().count() as i64;
            average_text_height += t.height;
        }
        if !texts.is_empty() {
            average_text_height /= texts.len() as f64;
        }
        let font_size_stats = FontSizeStats::new(&font_size_stats_raw);

        Ok(Page {
            number,
            odd_even,
            top,
            left,
            width,
            height,
            id,
            page_break_after: false,
            texts,
            imgs,
            left_margin,
            right_margin,
            id_used: false,
            textwidth,
            font_size_stats,
            document_font_stats: None,
            average_text_height,
            stats_left_min: 0.0,
            stats_left_max: 0.0,
            stats_indent_min: 0.0,
            stats_indent_max: 0.0,
            stats_right: 0.0,
            stats_margin_px: 0.0,
            contents: false,
            regions: Vec::new(),
        })
    }

    /// Port of `Page.is_empty` (reflow.py lines 850-854).
    pub fn is_empty(&self) -> bool {
        self.texts.is_empty() && self.imgs.is_empty()
    }

    /// Port of `Page.find_match` (reflow.py lines 856-882), returning the
    /// index of the first matching fragment on the same line as `frag`.
    /// The Python original has unreachable code after an unconditional
    /// `return t` inside the matching branch (a secondary "sorting can put
    /// parts of a line in the wrong order" check) - omitted here as dead.
    pub fn find_match(&self, frag: &Text) -> Option<usize> {
        let line_height = frag.bottom - frag.top;
        for (idx, t) in self.texts.iter().enumerate() {
            if t.id == frag.id {
                continue;
            }
            let top = frag.top.min(t.top);
            let bot = frag.bottom.max(t.bottom);
            if bot - top < line_height * HEIGHT_FACTOR
                && ((frag.top == t.top || frag.bottom == t.bottom)
                    || (frag.top < t.top && frag.bottom > t.top + BOTTOM_FACTOR)
                    || (frag.top < t.top && frag.bottom + BOTTOM_FACTOR > t.bottom)
                    || (t.top < frag.top && t.bottom > frag.top + BOTTOM_FACTOR)
                    || (t.top < frag.top && t.bottom + BOTTOM_FACTOR > frag.bottom))
            {
                return Some(idx);
            }
        }
        None
    }

    /// Port of `Page.join_fragments` (reflow.py lines 884-914): join
    /// same-line fragments into single `Text`s.
    pub fn join_fragments(&mut self) {
        let mut match_found = true;
        let mut tind = 0usize;
        while match_found {
            match_found = false;
            let mut removed_id: Option<u64> = None;
            while tind < self.texts.len() {
                let frag_id = self.texts[tind].id;
                if let Some(match_idx) = self.find_match(&self.texts[tind]) {
                    let match_id = self.texts[match_idx].id;
                    let frag_left = self.texts[tind].left;
                    let match_left = self.texts[match_idx].left;
                    let (base_id, other_id) = if frag_left > match_left {
                        (match_id, frag_id)
                    } else {
                        (frag_id, match_id)
                    };
                    let base_pos = self
                        .texts
                        .iter()
                        .position(|t| t.id == base_id)
                        .expect("base exists");
                    let other_pos = self
                        .texts
                        .iter()
                        .position(|t| t.id == other_id)
                        .expect("other exists");
                    let other_snapshot = self.texts[other_pos].clone();
                    let base_right = self.texts[base_pos].right;
                    let mut do_coalesce = true;
                    if other_snapshot.left < base_right {
                        let base_text = self.texts[base_pos].text_as_string.clone();
                        if other_snapshot.text_as_string.chars().count() == 1
                            && !base_text.is_empty()
                            && other_snapshot.left + other_snapshot.width > base_right
                            && base_text.chars().last()
                                == other_snapshot.text_as_string.chars().next()
                        {
                            // Overlapping same character: ignore it (still removed below).
                            do_coalesce = false;
                        }
                    }
                    if do_coalesce {
                        self.texts[base_pos].coalesce(
                            &other_snapshot,
                            self.number,
                            self.left_margin,
                            self.right_margin,
                        );
                    }
                    removed_id = Some(other_id);
                    break;
                }
                tind += 1;
            }
            if let Some(id) = removed_id {
                match_found = true;
                self.texts.retain(|t| t.id != id);
            }
        }
    }

    /// Port of `Page.check_centered` (reflow.py lines 916-997): detect
    /// centered/right-aligned text and heuristic heading levels.
    pub fn check_centered(&mut self, stats: &DocStats) {
        let mut first = true;
        self.contents = false;
        let left_max = self.stats_left_max;
        let indent_min = self.stats_indent_min;
        let indent_max = self.stats_indent_max;
        let width = self.width;

        let m = self.texts.len();
        for i in 0..m {
            let (last_left, last_right, right, top, bottom) = {
                let t = &self.texts[i];
                (t.last_left, t.last_right, t.right, t.top, t.bottom)
            };
            let lmargin = last_left;
            let rmargin = if bottom - top > stats.line_space * 2.0 {
                width - last_right
            } else {
                width - right
            };
            let xmargin = if i > 0 {
                self.texts[i - 1].last_left
            } else {
                -1.0
            };
            let ymargin = if i + 1 < m {
                self.texts[i + 1].last_left
            } else {
                -1.0
            };

            if contents_re().is_match(&self.texts[i].text_as_string) {
                self.contents = true;
                self.texts[i].tag = "h2".to_string();
            }

            if (lmargin < indent_min || lmargin > indent_max)
                && lmargin > left_max
                && lmargin != xmargin
                && lmargin != ymargin
                && lmargin >= rmargin - rmargin * CENTER_FACTOR
                && lmargin <= rmargin + rmargin * CENTER_FACTOR
                && !self.texts[i].raw.contains(r#""float:right""#)
            {
                self.texts[i].align = Align::Center;
            } else if lmargin > indent_max && lmargin > rmargin * RIGHT_FACTOR {
                self.texts[i].align = Align::Right;
            }

            if !self.contents {
                let s = self.texts[i].text_as_string.clone();
                let align = self.texts[i].align.clone();
                if roman_numerals_re().is_match(&s) {
                    self.texts[i].tag = "h3".to_string();
                } else if first && align == Align::Center && centered_digits_re().is_match(&s) {
                    self.texts[i].tag = "h2".to_string();
                } else if part_heading_re().is_match(&s) {
                    self.texts[i].tag = "h1".to_string();
                } else if chapter_heading_re().is_match(&s)
                    || prologue_epilogue_re().is_match(&s)
                    || (first && align == Align::Center && lowercase_heading_re().is_match(&s))
                    || (first && allcaps_heading_re().is_match(&s))
                {
                    self.texts[i].tag = "h2".to_string();
                }
            }
            first = false;
        }

        for img in &mut self.imgs {
            let lmargin = img.left;
            let rmargin = width - img.right;
            if lmargin > left_max
                && lmargin != indent_min
                && lmargin >= rmargin - rmargin * CENTER_FACTOR
                && lmargin <= rmargin + rmargin * CENTER_FACTOR
            {
                img.align = Align::Center;
            }
        }
    }

    /// Port of `Page.coalesce_paras` (reflow.py lines 999-1086): join lines
    /// into paragraphs.
    pub fn coalesce_paras(&mut self, stats: &DocStats, opts: &ReflowOpts) {
        let left_min = self.stats_left_min;
        let left_max = self.stats_left_max;
        let indent_min = self.stats_indent_min;
        let indent_max = self.stats_indent_max;
        let page_width = self.width;
        let margin_px = self.stats_margin_px;

        let mut match_found = true;
        let mut last_frag_id: Option<u64> = None;
        let mut tind = 0usize;
        while match_found {
            match_found = false;
            let mut removed_id: Option<u64> = None;
            while tind < self.texts.len() {
                let frag_id = self.texts[tind].id;
                if self.texts[tind].is_spaces() {
                    removed_id = Some(frag_id);
                    break;
                }
                let can_merge_now = match last_frag_id {
                    Some(lf_id) if lf_id != frag_id => {
                        self.texts.iter().position(|t| t.id == lf_id).map(|lf_pos| {
                            can_merge(
                                &self.texts[lf_pos],
                                &self.texts[tind],
                                stats,
                                left_min,
                                indent_min,
                                page_width,
                                opts.unwrap_factor,
                            )
                        }) == Some(true)
                    }
                    _ => false,
                };
                if can_merge_now {
                    let lf_id = last_frag_id.expect("checked above");
                    let lf_pos = self
                        .texts
                        .iter()
                        .position(|t| t.id == lf_id)
                        .expect("last_frag exists");
                    let frag_snapshot = self.texts[tind].clone();
                    self.texts[lf_pos].coalesce(
                        &frag_snapshot,
                        self.number,
                        self.left_margin,
                        self.right_margin,
                    );
                    self.texts[lf_pos].last_left = frag_snapshot.left;
                    self.texts[lf_pos].last_right = frag_snapshot.right;
                    self.texts[lf_pos].final_width = frag_snapshot.final_width;
                    removed_id = Some(frag_id);
                    break;
                } else if self.texts[tind].tag == "p" {
                    let (findented, falign, fleft, facw) = {
                        let f = &self.texts[tind];
                        (
                            f.indented,
                            f.align.clone(),
                            f.left,
                            f.average_character_width,
                        )
                    };
                    if findented == 0 && falign != Align::Center && fleft > left_max + facw {
                        if indent_min <= fleft && fleft <= indent_max {
                            self.texts[tind].indented = 1;
                        } else {
                            self.texts[tind].margin_left =
                                py_round(((fleft - left_min) / margin_px) + 0.5) as i64;
                        }
                    }
                    if let Some(lf_id) = last_frag_id {
                        if let Some(lf_pos) = self.texts.iter().position(|t| t.id == lf_id) {
                            let lf_bottom = self.texts[lf_pos].bottom;
                            let bottom = self.texts[tind].bottom;
                            if stats.para_space > 0.0
                                && bottom - lf_bottom > stats.para_space * SECTION_FACTOR
                            {
                                self.texts[tind].blank_line_before = true;
                            }
                        }
                    }
                }
                last_frag_id = Some(frag_id);
                tind += 1;
            }
            if let Some(id) = removed_id {
                match_found = true;
                self.texts.retain(|t| t.id != id);
            }
        }
    }

    /// Port of `Page.remove_head_foot_regex` (reflow.py lines 1088-1121).
    /// User-supplied regexes that fail to compile are logged and skipped
    /// rather than causing a crash (docs/FAULT_TOLERANCE.md: never panic on
    /// malformed/untrusted input, including user-supplied option values).
    pub fn remove_head_foot_regex(&mut self, opts: &ReflowOpts, log: &mut ReflowLog) {
        if !opts.pdf_header_regex.is_empty() && !self.texts.is_empty() {
            match Regex::new(&format!("^(?:{})", opts.pdf_header_regex)) {
                Ok(re) => {
                    for _ in 0..LINE_SCAN_COUNT {
                        if self.texts.is_empty() {
                            break;
                        }
                        if re.is_match(&self.texts[0].text_as_string) {
                            self.texts.remove(0);
                        }
                    }
                }
                Err(e) => log.warn(format!("invalid pdf_header_regex: {e}")),
            }
        }
        if !opts.pdf_footer_regex.is_empty() && !self.texts.is_empty() {
            match Regex::new(&format!("^(?:{})", opts.pdf_footer_regex)) {
                Ok(re) => {
                    for _ in 0..LINE_SCAN_COUNT {
                        if self.texts.is_empty() {
                            break;
                        }
                        let last = self.texts.len() - 1;
                        if re.is_match(&self.texts[last].text_as_string) {
                            self.texts.remove(last);
                        }
                    }
                }
                Err(e) => log.warn(format!("invalid pdf_footer_regex: {e}")),
            }
        }
    }

    /// Port of `Page.create_page_format` (reflow.py lines 1123-1134).
    pub fn create_page_format(
        &mut self,
        font_map: &IndexMap<String, Font>,
        opts: &ReflowOpts,
        log: &mut ReflowLog,
    ) {
        self.update_font_sizes(font_map);
        self.join_fragments();
        self.remove_head_foot_regex(opts, log);
    }

    /// Port of `Page.find_margins` (reflow.py lines 1136-1184; the
    /// `#### NOT IMPLEMENTED ####` tail starting at line 1185 - unreachable
    /// upstream, guarded by a `return` two lines above it - is not ported;
    /// see the module doc comment).
    pub fn find_margins(
        &mut self,
        tops: &mut IndexMap<FKey, i64>,
        indents: &mut IndexMap<FKey, i64>,
        line_spaces: &mut IndexMap<FKey, i64>,
        bottoms: &mut IndexMap<FKey, i64>,
        rights: &mut IndexMap<FKey, i64>,
    ) {
        let mut max_bot = 0.0f64;
        let mut max_right = 0.0f64;
        let mut max_space = 0.0f64;
        let mut last_top = 0.0f64;
        let mut first = true;
        for text in &mut self.texts {
            if text.left.round() != text.left {
                text.left = text.left.round();
            }
            if text.right.round() != text.right {
                text.right = text.right.round();
            }
            let top = text.top;
            let left = text.left;
            let right = text.right;
            if first {
                *tops.entry(FKey::new(top)).or_insert(0) += 1;
                first = false;
            } else {
                let space = (top - last_top).abs();
                if text.height <= space {
                    *line_spaces.entry(FKey::new(space)).or_insert(0) += 1;
                } else if max_space == 0.0 {
                    max_space = space;
                }
            }
            last_top = top;
            max_bot = max_bot.max(text.bottom);
            max_right = max_right.max(right);
            *indents.entry(FKey::new(left)).or_insert(0) += 1;
        }

        if max_bot > 0.0 {
            *bottoms.entry(FKey::new(max_bot)).or_insert(0) += 1;
        }
        if max_right > 0.0 {
            *rights.entry(FKey::new(max_right)).or_insert(0) += 1;
        }
        if max_space > 0.0 && line_spaces.is_empty() {
            *line_spaces.entry(FKey::new(max_space)).or_insert(0) += 1;
        }
    }

    /// Port of `Page.update_font_sizes` (reflow.py lines 1310-1315).
    pub fn update_font_sizes(&mut self, font_map: &IndexMap<String, Font>) {
        for text in &mut self.texts {
            if let Some(font) = font_map.get(&text.font_id) {
                text.font_size_em = font.size_em;
            }
            if text.font_size_em != 0.0 && text.font_size_em != 1.0 {
                text.raw = format!(
                    r#"<span style="font-size:{}em">{}</span>"#,
                    format_py_float(text.font_size_em),
                    text.raw
                );
            }
        }
    }

    /// Port of `Page.second_pass` (reflow.py lines 1317-1349; the
    /// `NOT IMPLEMENTED` region-linearization tail is not ported, see the
    /// module doc comment).
    pub fn second_pass(&mut self, stats: &DocStats, opts: &ReflowOpts) {
        if self.odd_even != 0 {
            self.stats_left_min = stats.left_min_odd;
            self.stats_left_max = stats.left_max_odd;
            self.stats_indent_min = stats.indent_min_odd;
            self.stats_indent_max = stats.indent_max_odd;
        } else {
            self.stats_left_min = stats.left_min_even;
            self.stats_left_max = stats.left_max_even;
            self.stats_indent_min = stats.indent_min_even;
            self.stats_indent_max = stats.indent_max_even;
        }
        self.stats_right = stats.right;
        self.stats_margin_px = stats.margin_px;

        self.coalesce_paras(stats, opts);
        self.check_centered(stats);
    }

    /// Port of `Page.to_html` (reflow.py lines 1351-1419).
    pub fn to_html(&mut self) -> Vec<String> {
        let mut ans: Vec<String> = Vec::new();
        let mut iind = 0usize;
        let ilen = self.imgs.len();

        for ti in 0..self.texts.len() {
            let text_top = self.texts[ti].top;
            let itop = if iind < ilen {
                self.imgs[iind].top
            } else {
                999_999.0
            };
            if itop <= text_top {
                let (img_align, img_html) = {
                    let img = &self.imgs[iind];
                    (img.align.clone(), img.to_html())
                };
                let mut s = "<p".to_string();
                if img_align == Align::Center {
                    s.push_str(r#" style="text-align:center""#);
                }
                if !self.id_used {
                    self.id_used = true;
                    s.push_str(&format!(r#" id="page_{}""#, self.number));
                }
                s.push('>');
                s.push_str(&img_html);
                s.push_str("</p>");
                ans.push(s);
                iind += 1;
            }

            let (tag, align, indented, margin_left, margin_right, blank_before, blank_after, html) = {
                let t = &self.texts[ti];
                (
                    t.tag.clone(),
                    t.align.clone(),
                    t.indented,
                    t.margin_left,
                    t.margin_right,
                    t.blank_line_before,
                    t.blank_line_after,
                    t.to_html(),
                )
            };
            if blank_before {
                ans.push(r#"<p style="text-align:center">&#160;</p>"#.to_string());
            }
            let mut s = format!("<{tag}");
            if !self.id_used {
                self.id_used = true;
                s.push_str(&format!(r#" id="page_{}""#, self.number));
            }
            if align == Align::Center {
                s.push_str(r#" style="text-align:center""#);
            } else if align == Align::Right {
                s.push_str(r#" style="text-align:right""#);
            } else if indented > 0 {
                s.push_str(&format!(r#" style="text-indent:{indented}em""#));
            } else if margin_left > 0 {
                s.push_str(&format!(r#" style="margin-left:{margin_left}em""#));
            } else if margin_right > 0 {
                s.push_str(&format!(r#" style="margin-right:{margin_right}em""#));
            }
            s.push('>');
            s.push_str(&html);
            s.push_str(&format!("</{tag}>"));
            ans.push(s);
            if blank_after {
                ans.push(r#"<p style="text-align:center">&#160;</p>"#.to_string());
            }
        }

        while iind < ilen {
            let (img_align, img_html) = {
                let img = &self.imgs[iind];
                (img.align.clone(), img.to_html())
            };
            let mut s = "<p".to_string();
            if img_align == Align::Center {
                s.push_str(r#" style="text-align:center""#);
            }
            if !self.id_used {
                self.id_used = true;
                s.push_str(&format!(r#" id="page_{}""#, self.number));
            }
            s.push('>');
            s.push_str(&img_html);
            s.push_str("</p>");
            ans.push(s);
            iind += 1;
        }

        ans
    }
}

fn can_merge(
    first_text: &Text,
    second_text: &Text,
    stats: &DocStats,
    left_min: f64,
    indent_min: f64,
    page_width: f64,
    unwrap_factor: f64,
) -> bool {
    let same_left = first_text.last_left - SAME_INDENT <= second_text.left
        && second_text.left <= first_text.last_left + SAME_INDENT;
    let structural = (second_text.left < left_min + second_text.average_character_width
        && (same_left
            || (second_text.left < first_text.last_left
                && (first_text.indented > 0 || first_text.raw.contains(r#""float:left""#)))))
        || (same_left && first_text.indented == 0 && second_text.left >= indent_min)
        || (same_left && first_text.indented == second_text.indented && second_text.indented > 1)
        || (second_text.left >= first_text.last_left && second_text.bottom <= first_text.bottom);

    structural
        && !second_text.raw.contains("href=")
        && !first_text.raw.contains(r#""float:right""#)
        && first_text.bottom + stats.line_space + (stats.line_space * LINE_FACTOR)
            >= second_text.bottom
        && first_text.final_width > page_width * unwrap_factor
        && !adjacent_quotes(&first_text.text_as_string, &second_text.text_as_string)
}

fn contents_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^\s*(table of )?contents\s*$").expect("static regex"))
}

fn roman_numerals_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*[iIxXvV]+\s*$").expect("static regex"))
}

fn centered_digits_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*\d+\s*$").expect("static regex"))
}

fn part_heading_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^\s*part\s[A-Za-z0-9]+$").expect("static regex"))
}

fn chapter_heading_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^\s*chapter\s").expect("static regex"))
}

/// Port of `r'(?i)^\s*prologue|epilogue\s*$'` (reflow.py line 983). Note
/// this is *not* `(?i)^\s*(prologue|epilogue)\s*$` - regex alternation has
/// the lowest precedence, so the Python source actually means "starts with
/// optional whitespace then 'prologue'" OR "ends with 'epilogue' then
/// optional whitespace" (each disjunct independently anchored at position 0
/// by `re.match`'s implicit start-anchor, which is why the second branch
/// gets an explicit `^` here that Rust's non-anchoring `is_match` needs but
/// Python's `re.match` didn't).
fn prologue_epilogue_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^\s*prologue|^epilogue\s*$").expect("static regex"))
}

fn lowercase_heading_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^\s*[a-z -]+\s*$").expect("static regex"))
}

fn allcaps_heading_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*[A-Z -]+\s*$").expect("static regex"))
}

// ==========================================================================
// PdfDocument (reflow.py lines 1422-2093: `PDFDocument`)
// ==========================================================================

/// Port of `PDFDocument` (reflow.py lines 1422-2093).
///
/// Differs from the Python original in one deliberate way: Python's
/// `__init__` unconditionally calls `self.render()` as its last step,
/// which hard-codes writing `index.html` into the process's current
/// working directory as a side effect of *construction*. That is neither
/// testable nor idiomatic Rust (and reaches for implicit CWD file I/O,
/// which `docs/FAULT_TOLERANCE.md` steers away from for anything that
/// isn't an explicit, caller-directed write). This port instead exposes
/// [`PdfDocument::render_html`] (pure, returns a `String`) and
/// [`PdfDocument::write_html`] (explicit target directory, returns
/// `io::Result`) as separate calls the caller makes after construction.
#[derive(Debug, Clone, Default)]
pub struct PdfDocument {
    pub stats: DocStats,
    pub fonts: Vec<Font>,
    pub font_map: IndexMap<String, Font>,
    pub pages: Vec<Page>,
    pub font_size_stats: FontSizeStats,
    pub font_sizes: IndexMap<String, i64>,
    pub tops: IndexMap<FKey, i64>,
    pub indents_odd: IndexMap<FKey, i64>,
    pub indents_even: IndexMap<FKey, i64>,
    pub line_spaces: IndexMap<FKey, i64>,
    pub bottoms: IndexMap<FKey, i64>,
    pub rights: IndexMap<FKey, i64>,
}

impl PdfDocument {
    /// Port of `PDFDocument.__init__` (reflow.py lines 1424-1495), minus
    /// the final `self.render()` call - see the struct doc comment.
    pub fn from_xml(
        xml: &str,
        opts: &mut ReflowOpts,
        log: &mut ReflowLog,
    ) -> Result<PdfDocument, ReflowError> {
        let xml_doc = Document::parse(xml)?;
        let root = xml_doc.root_element();
        let mut idc = IdGen::new();

        let mut fonts: Vec<Font> = Vec::new();
        let mut font_map: IndexMap<String, Font> = IndexMap::new();
        for spec in descendants_with_tag(root, "fontspec") {
            let f = Font::from_fontspec(&spec)?;
            font_map.insert(f.id.clone(), f.clone());
            fonts.push(f);
        }

        let mut pages: Vec<Page> = Vec::new();
        for page_node in descendants_with_tag(root, "page") {
            pages.push(Page::from_node(page_node, &font_map, opts, &mut idc)?);
        }

        let mut doc = PdfDocument {
            stats: DocStats::default(),
            fonts,
            font_map,
            pages,
            font_size_stats: FontSizeStats::default(),
            font_sizes: IndexMap::new(),
            tops: IndexMap::new(),
            indents_odd: IndexMap::new(),
            indents_even: IndexMap::new(),
            line_spaces: IndexMap::new(),
            bottoms: IndexMap::new(),
            rights: IndexMap::new(),
        };

        doc.collect_font_statistics();

        {
            let PdfDocument {
                pages,
                font_map,
                font_size_stats,
                ..
            } = &mut doc;
            for page in pages.iter_mut() {
                page.document_font_stats = Some(font_size_stats.clone());
                page.create_page_format(font_map, opts, log);
            }
        }

        if opts.pdf_header_skip < 0.0 || opts.pdf_footer_skip < 0.0 {
            doc.find_header_footer(opts);
        }
        if opts.pdf_header_skip > 0.0 || opts.pdf_footer_skip > 0.0 {
            doc.remove_header_footer(opts);
        }

        {
            let PdfDocument {
                pages,
                tops,
                indents_odd,
                indents_even,
                line_spaces,
                bottoms,
                rights,
                ..
            } = &mut doc;
            for page in pages.iter_mut() {
                let indents = if page.odd_even != 0 {
                    &mut *indents_odd
                } else {
                    &mut *indents_even
                };
                page.find_margins(tops, indents, line_spaces, bottoms, rights);
            }
        }

        doc.setup_stats();

        {
            let PdfDocument { pages, stats, .. } = &mut doc;
            for page in pages.iter_mut() {
                page.second_pass(stats, opts);
            }
        }

        doc.merge_pages(opts);

        Ok(doc)
    }

    /// Port of `PDFDocument.collect_font_statistics` (reflow.py lines
    /// 1497-1527).
    pub fn collect_font_statistics(&mut self) {
        let mut summed_ratios: IndexMap<FKey, f64> = IndexMap::new();
        let mut font_sizes: IndexMap<String, i64> = IndexMap::new();
        for p in &self.pages {
            for (&sz, &ratio) in p.font_size_stats.ratios.iter() {
                *summed_ratios.entry(sz).or_insert(0.0) += ratio;
            }
            for text in &p.texts {
                *font_sizes.entry(text.font_id.clone()).or_insert(0) += 1;
            }
        }

        let total: f64 = summed_ratios.values().sum();
        let mut most_common_size = -1.0;
        let mut chars_at_most_common_size = 0.0f64;
        let mut ratios = IndexMap::new();
        for (&sz, &val) in summed_ratios.iter() {
            if val >= chars_at_most_common_size {
                most_common_size = sz.value();
                chars_at_most_common_size = val;
            }
            ratios.insert(sz, if total > 0.0 { val / total } else { 0.0 });
        }
        self.font_size_stats = FontSizeStats {
            ratios,
            most_common_size,
            chars_at_most_common_size: chars_at_most_common_size as i64,
        };
        self.font_sizes = font_sizes;

        let mut fcount = 0i64;
        let mut f_ind: Option<String> = None;
        for (id, &count) in self.font_sizes.iter() {
            if fcount < count {
                fcount = count;
                f_ind = Some(id.clone());
            }
        }

        // NOTE: upstream uses the winning font's *id string parsed as an
        // integer* directly as a list index into `self.fonts` (all
        // `<fontspec>` occurrences across every page, duplicates
        // included) - not a lookup by id. Since font ids are typically
        // small sequential per-page integers ("0", "1", "2", ...) this
        // mostly "works" for single-font-table documents but is fragile
        // for documents where per-page font tables diverge; faithfully
        // replicated here, but with a bounds/parse-failure fallback to
        // 12.0 instead of the `IndexError` upstream would raise (per
        // docs/FAULT_TOLERANCE.md: never panic on malformed input).
        self.stats.font_size = if !self.fonts.is_empty() {
            f_ind
                .as_ref()
                .and_then(|id| id.parse::<usize>().ok())
                .and_then(|idx| self.fonts.get(idx))
                .map(|f| f.size)
                .unwrap_or(12.0)
        } else {
            12.0
        };
        self.stats.margin_px = (self.stats.font_size * 16.0 / 12.0).max(1.0);

        let font_size = self.stats.font_size;
        for f in &mut self.fonts {
            f.size_em = py_round_to(f.size / font_size, 2);
        }
        for f in self.font_map.values_mut() {
            f.size_em = py_round_to(f.size / font_size, 2);
        }
    }

    /// Port of `PDFDocument.setup_stats` (reflow.py lines 1529-1752).
    pub fn setup_stats(&mut self) {
        let mut tcount = 0i64;
        for (&t, &count) in self.tops.iter() {
            if tcount < count {
                tcount = count;
                self.stats.top = t.value();
            }
        }

        set_indents(&mut self.stats, &mut self.indents_odd, true);
        set_indents(&mut self.stats, &mut self.indents_even, false);

        self.stats.right = self.rights.keys().map(|k| k.value()).fold(0.0, f64::max);

        if self.stats.indent_min_odd - self.stats.left_min_odd
            > (self.stats.right - self.stats.left_min_odd) * 0.10
        {
            self.stats.indent_min_odd = self.stats.left_min_odd;
            self.stats.indent_max_odd = self.stats.left_min_odd;
            self.stats.indent_min_even = self.stats.left_min_even;
            self.stats.indent_max_even = self.stats.left_min_even;
        }

        self.stats.line_space = -1.0;
        self.stats.para_space = -1.0;

        let mut line_k = 0.0f64;
        let mut line_c = 0i64;
        let mut count = self.line_spaces.len() as i64;
        while count > 0 {
            let (c, k) = find_line_space(&self.line_spaces, line_c);
            if line_c <= 0 {
                line_c = c;
            }
            if line_k <= 0.0 {
                line_k = k;
            } else if (line_k - k).abs() <= SAME_SPACE {
                line_k = line_k.max(k);
                line_c = line_c.min(c);
            } else {
                break;
            }
            count -= 1;
        }

        let mut para_c = line_c - 1;
        let mut para_k = 0.0f64;
        let mut count = self.line_spaces.len() as i64;
        while count > 0 {
            let (c, k) = find_line_space(&self.line_spaces, para_c);
            if para_k <= 0.0 {
                para_k = k;
            }
            if (para_k - k).abs() <= SAME_SPACE {
                para_k = para_k.max(k);
                para_c = para_c.min(c);
            } else {
                break;
            }
            count -= 1;
        }

        if para_k == 0.0 || para_k == line_k {
            para_k = py_round(line_k * PARA_FACTOR);
        }
        if line_k > para_k {
            std::mem::swap(&mut line_k, &mut para_k);
        }

        self.stats.line_space = line_k;
        self.stats.para_space = if para_k > py_round(line_k * PARA_FACTOR) {
            py_round(line_k * PARA_FACTOR)
        } else {
            para_k
        };

        self.stats.bottom = self
            .bottoms
            .keys()
            .map(|k| k.value())
            .fold(self.stats.bottom, f64::max);
    }

    /// Port of `PDFDocument.find_header_footer` (reflow.py lines
    /// 1754-1895).
    pub fn find_header_footer(&mut self, opts: &mut ReflowOpts) {
        if (opts.pdf_header_skip >= 0.0 && opts.pdf_footer_skip >= 0.0) || self.pages.len() < 2 {
            return;
        }

        let scan_count = PAGE_SCAN_COUNT as i64;
        let mut head_text = vec![String::new(); LINE_SCAN_COUNT];
        let mut head_match = [0i64; LINE_SCAN_COUNT];
        let mut head_match1 = [0i64; LINE_SCAN_COUNT];
        let mut head_match2 = [0i64; LINE_SCAN_COUNT];
        let mut head_page: i64 = 0;
        let mut head_skip = 0.0f64;
        let mut foot_text = vec![String::new(); LINE_SCAN_COUNT];
        let mut foot_match = [0i64; LINE_SCAN_COUNT];
        let mut foot_match1 = [0i64; LINE_SCAN_COUNT];
        let mut foot_match2 = [0i64; LINE_SCAN_COUNT];
        let mut foot_page: i64 = 0;
        let mut foot_skip = 0.0f64;
        let mut fixed_head = String::new();
        let mut fixed_foot = String::new();

        let mut pages_to_scan = scan_count;
        for page in &self.pages {
            if opts.pdf_header_skip < 0.0 && !page.texts.is_empty() {
                for head_ind in 0..LINE_SCAN_COUNT {
                    if page.texts.len() < head_ind + 1
                        || page.texts[head_ind].top > page.height / 2.0
                    {
                        break;
                    }
                    let t = &page.texts[head_ind].text_as_string;
                    if head_text[head_ind].is_empty() {
                        head_text[head_ind] = t.clone();
                    } else if head_text[head_ind] == *t {
                        head_match[head_ind] += 1;
                        if head_page == 0 {
                            head_page = page.number;
                        }
                    } else if pagenum_text_re().is_match(t) {
                        head_match1[head_ind] += 1;
                        if head_page == 0 {
                            head_page = page.number;
                        }
                    } else if let Some(g1) = fixed_text_re().captures(t).and_then(|c| c.get(1)) {
                        let g1 = g1.as_str();
                        if !g1.is_empty() {
                            if fixed_head.is_empty() {
                                fixed_head = g1.to_string();
                            } else if fixed_head == g1 {
                                head_match2[head_ind] += 1;
                                if head_page == 0 {
                                    head_page = page.number;
                                }
                            }
                        }
                    }
                }
            }

            if opts.pdf_footer_skip < 0.0 && !page.texts.is_empty() {
                for foot_ind in 0..LINE_SCAN_COUNT {
                    if page.texts.len() < foot_ind + 1 {
                        break;
                    }
                    let idx = page.texts.len() - foot_ind - 1;
                    if page.texts[idx].top < page.height / 2.0 {
                        break;
                    }
                    let t = &page.texts[idx].text_as_string;
                    if foot_text[foot_ind].is_empty() {
                        foot_text[foot_ind] = t.clone();
                    } else if foot_text[foot_ind] == *t {
                        foot_match[foot_ind] += 1;
                        if foot_page == 0 {
                            foot_page = page.number;
                        }
                    } else if pagenum_text_re().is_match(t) {
                        foot_match1[foot_ind] += 1;
                        if foot_page == 0 {
                            foot_page = page.number;
                        }
                    } else if let Some(g1) = fixed_text_re().captures(t).and_then(|c| c.get(1)) {
                        let g1 = g1.as_str();
                        if !g1.is_empty() {
                            if fixed_foot.is_empty() {
                                fixed_foot = g1.to_string();
                            } else if fixed_foot == g1 {
                                foot_match2[foot_ind] += 1;
                                if foot_page == 0 {
                                    foot_page = page.number;
                                }
                            }
                        }
                    }
                }
            }

            pages_to_scan -= 1;
            if pages_to_scan < 1 {
                break;
            }
        }

        let pages_to_scan = if pages_to_scan > 0 {
            scan_count - pages_to_scan
        } else {
            scan_count
        };
        let pages_to_scan = pages_to_scan as f64 / 2.0;

        let mut head_ind = 0usize;
        for (i, item) in head_match.iter().enumerate().take(LINE_SCAN_COUNT) {
            if *item as f64 > pages_to_scan
                || head_match1[i] as f64 > pages_to_scan
                || head_match2[i] as f64 > pages_to_scan
            {
                head_ind = i;
            }
        }
        if let Some(hp) = usize::try_from(head_page)
            .ok()
            .and_then(|i| self.pages.get(i))
        {
            if !hp.texts.is_empty()
                && (head_match[head_ind] as f64 > pages_to_scan
                    || head_match1[head_ind] as f64 > pages_to_scan
                    || head_match2[head_ind] as f64 > pages_to_scan)
            {
                if let Some(t) = hp.texts.get(head_ind) {
                    head_skip = t.top + t.height + 1.0;
                }
            }
        }

        let mut foot_ind = 0usize;
        for (i, item) in foot_match.iter().enumerate().take(LINE_SCAN_COUNT) {
            if *item as f64 > pages_to_scan
                || foot_match1[i] as f64 > pages_to_scan
                || foot_match2[i] as f64 > pages_to_scan
            {
                foot_ind = i;
            }
        }
        if foot_page >= 0 && (foot_page as usize) < self.pages.len() {
            let fp = &self.pages[foot_page as usize];
            if !fp.texts.is_empty()
                && (foot_match[foot_ind] as f64 > pages_to_scan
                    || foot_match1[foot_ind] as f64 > pages_to_scan
                    || foot_match2[foot_ind] as f64 > pages_to_scan)
                && foot_ind < fp.texts.len()
            {
                let idx = fp.texts.len() - foot_ind - 1;
                foot_skip = fp.texts[idx].top - 1.0;
            }
        }

        if head_skip > 0.0 {
            opts.pdf_header_skip = head_skip;
        }
        if foot_skip > 0.0 {
            opts.pdf_footer_skip = foot_skip;
        }
    }

    /// Port of `PDFDocument.remove_header_footer` (reflow.py lines
    /// 1897-1909). Uses `Vec::retain` rather than the original's
    /// remove-and-restart loop; both produce the same result since the
    /// removal predicate does not depend on other elements.
    pub fn remove_header_footer(&mut self, opts: &ReflowOpts) {
        for page in &mut self.pages {
            page.texts.retain(|t| {
                !((opts.pdf_header_skip > 0.0 && t.top < opts.pdf_header_skip)
                    || (opts.pdf_footer_skip > 0.0 && t.top > opts.pdf_footer_skip))
            });
        }
    }

    /// Port of `PDFDocument.merge_pages` (reflow.py lines 1911-2040): merge
    /// paragraphs that continue across a page boundary, and mark pages that
    /// should force an early page-break to avoid orphans.
    ///
    /// Objects are tracked by stable id (`Page::number`, `Text::id`) rather
    /// than by Python-style object reference, since `self.pages` is a
    /// `Vec<Page>` whose indices shift as pages are removed.
    pub fn merge_pages(&mut self, opts: &ReflowOpts) {
        let min_top = self.stats.top;
        let max_bottom = self.stats.bottom;
        let orphan_space = max_bottom - ORPHAN_LINES * self.stats.line_space;
        let mut save_bottom = 0.0f64;
        let mut pind = 0usize;
        let mut save_candidate: Option<i64> = None;

        let mut merge_done = true;
        while merge_done {
            merge_done = false;
            let mut merged_page_number: Option<i64> = None;
            let mut merged_text_id: Option<u64> = None;
            let mut candidate_number: Option<i64> = save_candidate;
            save_candidate = None;

            while pind < self.pages.len() {
                let stats_left_min = self.pages[pind].stats_left_min;
                let has_texts = !self.pages[pind].texts.is_empty();
                let mut did_break = false;

                if has_texts {
                    if let Some(cand_num) = candidate_number {
                        if let Some(cand_pos) = self.pages.iter().position(|p| p.number == cand_num)
                        {
                            let last_line_bottom =
                                self.pages[cand_pos].texts.last().map(|t| t.bottom);
                            let last_line_raw =
                                self.pages[cand_pos].texts.last().map(|t| t.raw.clone());
                            let last_line_text = self.pages[cand_pos]
                                .texts
                                .last()
                                .map(|t| t.text_as_string.clone());
                            let last_line_final_width =
                                self.pages[cand_pos].texts.last().map(|t| t.final_width);
                            let last_line_top = self.pages[cand_pos].texts.last().map(|t| t.top);
                            let candidate_textwidth = self.pages[cand_pos].textwidth;

                            if let Some(llb) = last_line_bottom {
                                let page_indented0 = self.pages[pind].texts[0].indented == 0;
                                if llb > orphan_space && page_indented0 {
                                    let merged_text_snapshot = self.pages[pind].texts[0].clone();
                                    let top = merged_text_snapshot.top;
                                    let last_spare =
                                        candidate_textwidth - last_line_final_width.unwrap_or(0.0);
                                    let merged_len_word = leading_word_re()
                                        .captures(&merged_text_snapshot.text_as_string)
                                        .and_then(|c| c.get(1))
                                        .map(|m| {
                                            m.as_str().chars().count() as f64
                                                * merged_text_snapshot.average_character_width
                                        });
                                    let mut merged_len = merged_len_word.unwrap_or(0.0);
                                    let last_line_lc = last_line_text.clone().unwrap_or_default();
                                    if ends_lowercase_re().is_match(&last_line_lc)
                                        || starts_lowercase_re()
                                            .is_match(&merged_text_snapshot.text_as_string)
                                    {
                                        merged_len = merged_text_snapshot.right;
                                    }

                                    let page_avg_height = self.pages[pind].average_text_height;
                                    let merges = top <= min_top + page_avg_height
                                        && merged_text_snapshot.tag == "p"
                                        && !merged_text_snapshot.raw.contains("href=")
                                        && merged_text_snapshot.left
                                            < stats_left_min
                                                + merged_text_snapshot.average_character_width
                                        && last_spare <= merged_len
                                        && !(last_line_raw
                                            .as_deref()
                                            .unwrap_or("")
                                            .contains(r#""float:right""#)
                                            && merged_text_snapshot
                                                .raw
                                                .contains(r#""float:right""#))
                                        && !adjacent_quotes(
                                            &last_line_lc,
                                            &merged_text_snapshot.text_as_string,
                                        );

                                    if merges {
                                        merge_done = true;
                                        save_bottom = if self.pages[pind].texts.len() == 1 {
                                            merged_text_snapshot.bottom
                                        } else {
                                            0.0
                                        };
                                        let new_top =
                                            last_line_top.unwrap_or(0.0) + page_avg_height;
                                        self.pages[pind].texts[0].top = new_top;
                                        self.pages[pind].texts[0].bottom =
                                            new_top + merged_text_snapshot.height;
                                        merged_page_number = Some(self.pages[pind].number);
                                        merged_text_id = Some(merged_text_snapshot.id);
                                        did_break = true;
                                    } else {
                                        if self.pages[pind].texts[0].top
                                            > self.stats.top + self.stats.line_space
                                        {
                                            self.pages[pind].texts[0].blank_line_after = true;
                                        }
                                        candidate_number = None;
                                    }
                                }
                            }
                        }
                    }

                    if !did_break {
                        let (last_bottom, last_text_as_string, last_final_width) = {
                            let t = self.pages[pind].texts.last().expect("has_texts");
                            (t.bottom, t.text_as_string.clone(), t.final_width)
                        };
                        let imgs_empty = self.pages[pind].imgs.is_empty();
                        let last_img_bottom = self.pages[pind].imgs.last().map(|i| i.bottom);
                        if last_bottom < orphan_space
                            && (imgs_empty || last_img_bottom.unwrap_or(0.0) < orphan_space)
                        {
                            if pind + 1 < self.pages.len() {
                                let next_imgs_empty = self.pages[pind + 1].imgs.is_empty();
                                let next_first_img_height =
                                    self.pages[pind + 1].imgs.first().map(|i| i.height);
                                let next_texts_empty = self.pages[pind + 1].texts.is_empty();
                                let next_first_text_top =
                                    self.pages[pind + 1].texts.first().map(|t| t.top);
                                let next_first_img_top =
                                    self.pages[pind + 1].imgs.first().map(|i| i.top);
                                let ok = next_imgs_empty
                                    || (next_first_img_height.unwrap_or(0.0) < orphan_space
                                        && (next_texts_empty
                                            || next_first_text_top.unwrap_or(0.0)
                                                > next_first_img_top.unwrap_or(0.0)));
                                if ok {
                                    self.pages[pind].page_break_after = true;
                                }
                            }
                        } else if ends_lowercase_comma_space_re().is_match(&last_text_as_string)
                            || last_final_width > self.pages[pind].width * opts.unwrap_factor
                        {
                            candidate_number = Some(self.pages[pind].number);
                        }
                    }
                } else {
                    candidate_number = None;
                }

                if did_break {
                    break;
                }
                pind += 1;
            }

            if merge_done {
                let merged_page_number =
                    merged_page_number.expect("merge_done implies merged page set");
                let merged_text_id = merged_text_id.expect("merge_done implies merged text set");
                let cand_num = candidate_number.expect("merge_done implies candidate set");

                let merged_pos = self
                    .pages
                    .iter()
                    .position(|p| p.number == merged_page_number)
                    .expect("merged page exists");
                let cand_pos = self
                    .pages
                    .iter()
                    .position(|p| p.number == cand_num)
                    .expect("candidate page exists");

                let left_margin = self.pages[merged_pos].stats_left_min;
                let right_margin = self.pages[merged_pos].stats_right;
                let merged_text_pos = self.pages[merged_pos]
                    .texts
                    .iter()
                    .position(|t| t.id == merged_text_id)
                    .expect("merged text exists");
                let merged_text_snapshot = self.pages[merged_pos].texts[merged_text_pos].clone();

                let cand_last_idx = self.pages[cand_pos].texts.len() - 1;
                let cand_number = self.pages[cand_pos].number;
                self.pages[cand_pos].texts[cand_last_idx].coalesce(
                    &merged_text_snapshot,
                    cand_number,
                    left_margin,
                    right_margin,
                );
                self.pages[merged_pos].texts.remove(merged_text_pos);

                let mut candidate_number_local = Some(cand_num);
                if save_bottom != 0.0 {
                    self.pages[cand_pos].texts[cand_last_idx].bottom = save_bottom;
                    if self.pages[merged_pos].is_empty() && save_bottom < orphan_space {
                        self.pages[cand_pos].page_break_after = true;
                        candidate_number_local = None;
                    }
                }

                if self.pages[merged_pos].is_empty() {
                    save_candidate = candidate_number_local;
                    self.pages.remove(merged_pos);
                }
            }
        }
    }

    /// Port of `PDFDocument.render` (reflow.py lines 2067-2093), minus the
    /// file write - see the struct doc comment. `title` replaces reading
    /// `sys.argv[1]` (there is no meaningful CLI-argv analog inside a
    /// library call).
    pub fn render_html(&mut self, title: &str) -> String {
        let mut html = vec![
            r#"<?xml version="1.0" encoding="UTF-8"?>"#.to_string(),
            r#"<html xmlns="http://www.w3.org/1999/xhtml">"#.to_string(),
            "<head>".to_string(),
            format!("<title>{title}</title>"),
            r#"<meta content="PDF Reflow conversion" name="generator"/>"#.to_string(),
            "</head>".to_string(),
            "<body>".to_string(),
        ];
        for page in &mut self.pages {
            html.extend(page.to_html());
            if page.page_break_after {
                html.push(r#"<div style="page-break-after:always"></div>"#.to_string());
            }
        }
        html.push("</body>".to_string());
        html.push("</html>".to_string());
        let mut raw = html.join("\n");
        for (from, to) in [
            ("</strong><strong>", ""),
            ("</i><i>", ""),
            ("</em><em>", ""),
            ("</b><b>", ""),
            ("</strong> <strong>", " "),
            ("</i> <i>", " "),
            ("</em> <em>", " "),
            ("</b> <b>", " "),
        ] {
            raw = raw.replace(from, to);
        }
        raw
    }

    /// Render and write `index.html` into `dir`. Port of the file-write
    /// half of `PDFDocument.render` (reflow.py lines 2091-2092), split out
    /// as an explicit, caller-directed I/O step - see the struct doc
    /// comment. No `.unwrap()`/`.expect()`: I/O failure propagates as
    /// `Err`, per docs/FAULT_TOLERANCE.md.
    pub fn write_html(
        &mut self,
        dir: &std::path::Path,
        title: &str,
    ) -> std::io::Result<std::path::PathBuf> {
        let raw = self.render_html(title);
        let path = dir.join("index.html");
        std::fs::write(&path, raw.as_bytes())?;
        Ok(path)
    }
}

fn find_line_space(line_spaces: &IndexMap<FKey, i64>, skip: i64) -> (i64, f64) {
    let mut scount = 0i64;
    let mut soffset = 0.0f64;
    for (&s, &count) in line_spaces.iter() {
        if scount <= count && (skip <= 0 || count < skip) {
            scount = count;
            soffset = s.value();
        }
    }
    (scount, soffset)
}

fn find_indent(indents: &IndexMap<FKey, i64>) -> f64 {
    let mut icount = 0i64;
    let mut ioffset = 0.0f64;
    for (&i, &ii) in indents.iter() {
        if ii > 0 && icount <= ii {
            icount = ii;
            ioffset = i.value();
        }
    }
    ioffset
}

/// Port of `PDFDocument.setup_stats`'s nested `set_indents` closure
/// (reflow.py lines 1560-1654), extracted to a free function so it can
/// take disjoint `&mut` borrows of `self.stats` and `self.indents_odd`/
/// `self.indents_even` (a method taking `&mut self` for both cannot).
fn set_indents(stats: &mut DocStats, indents: &mut IndexMap<FKey, i64>, odd_even: bool) {
    let keys: Vec<FKey> = indents.keys().copied().collect();

    let mut left_k1 = 0.0f64;
    // `left_c` mirrors Python's `left_c` accumulator (reflow.py line
    // 1562), which is likewise computed and never subsequently read
    // there either - kept only for line-for-line fidelity.
    let mut left_k = find_indent(indents);
    for k in &keys {
        let kc = indents.get(k).copied().unwrap_or(0);
        if kc > 0 && (left_k - k.value()).abs() <= SAME_INDENT {
            left_k = left_k.min(k.value());
            left_k1 = left_k1.max(k.value());
            if let Some(v) = indents.get_mut(k) {
                *v = -kc;
            }
        }
    }

    let mut indent_k1 = 0.0f64;
    let mut indent_c = 0i64;
    let mut indent_k = find_indent(indents);
    for k in &keys {
        let kc = indents.get(k).copied().unwrap_or(0);
        if kc > 0 && (indent_k - k.value()).abs() <= SAME_INDENT {
            indent_k = indent_k.min(k.value());
            indent_k1 = indent_k1.max(k.value());
            indent_c += kc;
            if let Some(v) = indents.get_mut(k) {
                *v = -kc;
            }
        }
    }

    let mut third_k1 = 0.0f64;
    let mut third_c = 0i64;
    let third_k_initial = find_indent(indents);
    let mut third_k = third_k_initial;
    for k in &keys {
        let kc = indents.get(k).copied().unwrap_or(0);
        if kc > 0 && (third_k - k.value()).abs() <= SAME_INDENT {
            third_k = third_k.min(k.value());
            third_k1 = third_k1.max(k.value());
            third_c += kc;
            if let Some(v) = indents.get_mut(k) {
                *v = -kc;
            }
        }
    }

    if third_k > 0.0
        && third_k < indent_k
        && third_k > left_k
        && (third_c as f64) > indent_c as f64 / 2.0
    {
        indent_k = third_k;
        indent_k1 = third_k1;
    }

    if indent_k == 0.0 {
        indent_k = left_k1 + SAME_INDENT + 1.0;
        indent_k1 = indent_k;
    }

    if left_k > indent_k {
        std::mem::swap(&mut left_k, &mut indent_k);
        std::mem::swap(&mut left_k1, &mut indent_k1);
    }

    let left_max = if left_k1 != 0.0 { left_k1 } else { left_k };
    let indent_max = if indent_k1 != 0.0 {
        indent_k1
    } else {
        indent_k
    };

    if odd_even {
        stats.left_min_odd = left_k;
        stats.left_max_odd = left_max;
        stats.indent_min_odd = indent_k;
        stats.indent_max_odd = indent_max;
    } else {
        stats.left_min_even = left_k;
        stats.left_max_even = left_max;
        stats.indent_min_even = indent_k;
        stats.indent_max_even = indent_max;
    }
}

fn pagenum_text_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(?:.*\d+\s+\w+\s+\d+.*)|^(?:\s*\d+\s+.*)|^\s*[ivxlcIVXLC]+\s*$")
            .expect("static regex")
    })
}

fn fixed_text_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^(.+[^0-9])\d+\s*$").expect("static regex"))
}

fn leading_word_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^([^ ]+)\s").expect("static regex"))
}

fn ends_lowercase_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^.*[a-z,-]\s*$").expect("static regex"))
}

fn starts_lowercase_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*[a-z,-]").expect("static regex"))
}

fn ends_lowercase_comma_space_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^.*[a-z, ]$").expect("static regex"))
}

// ==========================================================================
// Tests
// ==========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_fontspec(xml: &str) -> Font {
        let doc = Document::parse(xml).expect("parses");
        let node = doc.root_element();
        Font::from_fontspec(&node).expect("valid fontspec")
    }

    #[test]
    fn font_parses_fontspec_attributes() {
        let f = parse_fontspec(r##"<fontspec id="0" size="12" family="Times" color="#000000"/>"##);
        assert_eq!(f.id, "0");
        assert_eq!(f.size, 12.0);
        assert_eq!(f.family.as_deref(), Some("Times"));
        assert_eq!(f.color.as_deref(), Some("#000000"));
    }

    #[test]
    fn font_missing_size_is_an_error_not_a_panic() {
        let doc = Document::parse(r#"<fontspec id="0" family="Times"/>"#).expect("parses");
        let node = doc.root_element();
        let err = Font::from_fontspec(&node).unwrap_err();
        assert!(matches!(err, ReflowError::MissingAttribute { .. }));
    }

    fn parse_text(xml: &str, font_map: &IndexMap<String, Font>) -> Text {
        let doc = Document::parse(xml).expect("parses");
        let node = doc.root_element();
        let mut idc = IdGen::new();
        Text::from_node(node, font_map, &mut idc).expect("valid text")
    }

    fn simple_font_map() -> IndexMap<String, Font> {
        let mut m = IndexMap::new();
        m.insert(
            "0".to_string(),
            Font {
                id: "0".to_string(),
                size: 12.0,
                size_em: 1.0,
                color: None,
                family: None,
            },
        );
        m
    }

    #[test]
    fn text_parses_geometry_and_content() {
        let fm = simple_font_map();
        let t = parse_text(
            r#"<text top="10" left="20" width="100" height="15" font="0">Hello world</text>"#,
            &fm,
        );
        assert_eq!(t.top, 10.0);
        assert_eq!(t.left, 20.0);
        assert_eq!(t.width, 100.0);
        assert_eq!(t.height, 15.0);
        assert_eq!(t.bottom, 25.0);
        assert_eq!(t.right, 120.0);
        assert_eq!(t.text_as_string, "Hello world");
        assert_eq!(t.raw, "Hello world");
    }

    #[test]
    fn text_preserves_nested_markup_in_raw_but_not_text_as_string() {
        let fm = simple_font_map();
        let t = parse_text(
            r#"<text top="10" left="20" width="100" height="15" font="0">Some <b>bold</b> word</text>"#,
            &fm,
        );
        assert_eq!(t.text_as_string, "Some bold word");
        assert_eq!(t.raw, "Some <b>bold</b> word");
    }

    #[test]
    fn text_escapes_special_characters_in_leading_text() {
        let fm = simple_font_map();
        let t = parse_text(
            r#"<text top="0" left="0" width="10" height="10" font="0">A &amp; B &lt; C</text>"#,
            &fm,
        );
        assert_eq!(t.text_as_string, "A & B < C");
        assert_eq!(t.raw, "A &amp; B &lt; C");
    }

    #[test]
    fn text_missing_font_id_is_an_error_when_font_map_nonempty() {
        let fm = simple_font_map();
        let doc =
            Document::parse(r#"<text top="0" left="0" width="10" height="10" font="99">x</text>"#)
                .expect("parses");
        let node = doc.root_element();
        let mut idc = IdGen::new();
        let err = Text::from_node(node, &fm, &mut idc).unwrap_err();
        assert!(matches!(err, ReflowError::UnknownFont(_)));
    }

    #[test]
    fn text_is_spaces_detects_whitespace_only_content() {
        let fm = simple_font_map();
        let spaces = parse_text(
            r#"<text top="0" left="0" width="10" height="10" font="0">   </text>"#,
            &fm,
        );
        assert!(spaces.is_spaces());
        let real = parse_text(
            r#"<text top="0" left="0" width="10" height="10" font="0">x</text>"#,
            &fm,
        );
        assert!(!real.is_spaces());
    }

    #[test]
    fn text_coalesce_joins_two_fragments_on_the_same_line() {
        let fm = simple_font_map();
        let mut first = parse_text(
            r#"<text top="10" left="0" width="50" height="12" font="0">Hello </text>"#,
            &fm,
        );
        let second = parse_text(
            r#"<text top="10" left="50" width="50" height="12" font="0">world</text>"#,
            &fm,
        );
        first.coalesce(&second, 1, 0.0, 1000.0);
        assert_eq!(first.text_as_string, "Hello world");
        assert!(first.raw.contains("Hello"));
        assert!(first.raw.contains("world"));
        assert_eq!(first.right, 100.0);
    }

    #[test]
    fn adjacent_quotes_detects_matching_quote_pairs() {
        assert!(adjacent_quotes("he said \"", "\" she replied"));
        assert!(!adjacent_quotes("hello", "world"));
        assert!(adjacent_quotes("close\u{2019}", "\u{2018}open"));
    }

    #[test]
    fn font_size_stats_computes_ratios_and_most_common_size() {
        let mut raw: IndexMap<FKey, i64> = IndexMap::new();
        raw.insert(FKey::new(10.0), 80);
        raw.insert(FKey::new(20.0), 20);
        let stats = FontSizeStats::new(&raw);
        assert_eq!(stats.most_common_size, 10.0);
        assert_eq!(stats.chars_at_most_common_size, 80);
        assert!((stats.ratios[&FKey::new(10.0)] - 0.8).abs() < 1e-9);
        assert!((stats.ratios[&FKey::new(20.0)] - 0.2).abs() < 1e-9);
    }

    #[test]
    fn interval_intersection_and_centered_in() {
        let a = Interval::new(0.0, 10.0);
        let b = Interval::new(5.0, 15.0);
        let i = a.intersection(&b);
        assert_eq!(i.left, 5.0);
        assert_eq!(i.right, 10.0);
        assert_eq!(i.width, 5.0);

        let parent = Interval::new(0.0, 100.0);
        let centered = Interval::new(40.0, 60.0);
        assert!(centered.centered_in(&parent));
        let off_center = Interval::new(0.0, 20.0);
        assert!(!off_center.centered_in(&parent));
    }

    fn make_text(id: u64, top: f64, left: f64, width: f64, height: f64) -> Text {
        Text {
            id,
            top,
            left,
            width,
            height,
            bottom: top + height,
            right: left + width,
            tag: "p".to_string(),
            indented: 0,
            margin_left: 0,
            margin_right: 0,
            last_left: left,
            last_right: left + width,
            final_width: width,
            align: Align::Left,
            blank_line_before: false,
            blank_line_after: false,
            font_id: "0".to_string(),
            font_size: 12.0,
            font_size_em: 1.0,
            color: None,
            font_family: None,
            text_as_string: "x".to_string(),
            raw: "x".to_string(),
            average_character_width: 5.0,
            indent_fraction: 0.0,
            width_fraction: 0.0,
            top_gap_ratio: None,
        }
    }

    #[test]
    fn column_add_computes_bounding_box_and_dedupes_by_id() {
        let mut col = Column::new();
        col.add(LineElem::Text(make_text(1, 0.0, 10.0, 50.0, 12.0)));
        col.add(LineElem::Text(make_text(2, 20.0, 5.0, 60.0, 12.0)));
        // Duplicate id: should not be added twice.
        col.add(LineElem::Text(make_text(1, 0.0, 10.0, 50.0, 12.0)));
        assert_eq!(col.len(), 2);
        assert_eq!(col.left, 5.0);
        assert_eq!(col.right, 65.0);
        assert_eq!(col.top, 0.0);
        assert_eq!(col.bottom, 32.0);
    }

    #[test]
    fn column_contains_respects_hfuzz() {
        let mut col = Column::new();
        col.add(LineElem::Text(make_text(1, 0.0, 100.0, 100.0, 12.0))); // left=100,right=200
                                                                        // width=100, HFUZZ=0.2 -> tolerance 20 either side.
        let inside = make_text(2, 0.0, 90.0, 100.0, 12.0); // left=90 > 100-20=80: inside
        assert!(col.contains(&LineElem::Text(inside)));
        let outside = make_text(3, 0.0, 50.0, 10.0, 12.0); // left=50 < 80: outside
        assert!(!col.contains(&LineElem::Text(outside)));
    }

    #[test]
    fn region_contains_matches_overlapping_column_layout() {
        let mut region = Region::new();
        let mut col = Column::new();
        col.add(LineElem::Text(make_text(1, 0.0, 0.0, 100.0, 12.0)));
        region.add(vec![col]);

        let mut similar = Column::new();
        similar.add(LineElem::Text(make_text(2, 0.0, 10.0, 100.0, 12.0)));
        assert!(region.contains(&[similar]));

        let mut different = Column::new();
        different.add(LineElem::Text(make_text(3, 0.0, 500.0, 50.0, 12.0)));
        assert!(!region.contains(&[different]));
    }

    #[test]
    fn region_linearize_splits_on_images_and_indent_gaps() {
        let mut region = Region::new();
        let mut col = Column::new();
        let mut t1 = make_text(1, 0.0, 0.0, 100.0, 12.0);
        t1.indent_fraction = 0.0;
        t1.top_gap_ratio = None;
        col.elements.push(LineElem::Text(t1));
        let img = Image {
            id: 2,
            top: 20.0,
            left: 0.0,
            width: 50.0,
            height: 50.0,
            bottom: 70.0,
            right: 50.0,
            src: "img.png".to_string(),
            align: Align::Left,
            indent_fraction: 0.0,
            width_fraction: 0.0,
            top_gap_ratio: None,
        };
        col.elements.push(LineElem::Image(img));
        region.columns.push(col);
        region.linearize();
        assert!(region.boxes.len() >= 2);
        assert!(matches!(region.boxes[0], RegionBox::Html(_)));
        assert!(region
            .boxes
            .iter()
            .any(|b| matches!(b, RegionBox::Image(_))));
    }

    fn synthetic_fontspecs() -> &'static str {
        r##"<fontspec id="0" size="10" family="Times" color="#000000"/>
           <fontspec id="1" size="16" family="Times" color="#000000"/>"##
    }

    /// Small, hand-written `pdftohtml -xml`-shaped fixture (matching the
    /// `//fontspec`/`//page`/descendant `text`/`image` XPath queries
    /// `PDFDocument.__init__` runs against real poppler output): two
    /// pages, a heading, two lines meant to coalesce into one paragraph,
    /// and a following paragraph that should stay separate.
    fn synthetic_doc_xml() -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<pdf2xml>
<page number="1" position="absolute" top="0" left="0" height="792" width="612">
{fonts}
<text top="50" left="200" width="210" height="20" font="1">Chapter One</text>
<text top="100" left="72" width="460" height="14" font="0">This paragraph starts here and it keeps going for quite a long while so the final width comfortably exceeds the unwrap threshold</text>
<text top="115" left="72" width="180" height="14" font="0">and finally concludes on this second physical line.</text>
<text top="140" left="72" width="300" height="14" font="0">A separate new paragraph starts here on its own line nicely done.</text>
</page>
<page number="2" position="absolute" top="0" left="0" height="792" width="612">
{fonts}
<text top="60" left="72" width="350" height="14" font="0">Another paragraph body line on page two of the document for good measure here.</text>
<text top="75" left="72" width="150" height="14" font="0">continued once more finally.</text>
</page>
</pdf2xml>"#,
            fonts = synthetic_fontspecs()
        )
    }

    #[test]
    fn pdf_document_parses_pages_and_fonts() {
        let xml = synthetic_doc_xml();
        let mut opts = ReflowOpts {
            pdf_header_skip: 0.0,
            pdf_footer_skip: 0.0,
            ..ReflowOpts::default()
        };
        let mut log = ReflowLog::default();
        let doc = PdfDocument::from_xml(&xml, &mut opts, &mut log).expect("parses synthetic doc");
        assert_eq!(doc.fonts.len(), 4); // two <fontspec> per page, two pages
        assert!(doc.pages.len() <= 2); // may merge to 1 page if page 2 fully absorbed
        assert!(!doc.pages.is_empty());
    }

    #[test]
    fn pdf_document_detects_chapter_heading() {
        let xml = synthetic_doc_xml();
        let mut opts = ReflowOpts {
            pdf_header_skip: 0.0,
            pdf_footer_skip: 0.0,
            ..ReflowOpts::default()
        };
        let mut log = ReflowLog::default();
        let doc = PdfDocument::from_xml(&xml, &mut opts, &mut log).expect("parses synthetic doc");
        let heading = doc.pages[0]
            .texts
            .iter()
            .find(|t| t.text_as_string.contains("Chapter One"))
            .expect("heading text survives");
        assert_eq!(heading.tag, "h2");
    }

    #[test]
    fn pdf_document_coalesces_wrapped_lines_into_one_paragraph() {
        let xml = synthetic_doc_xml();
        let mut opts = ReflowOpts {
            pdf_header_skip: 0.0,
            pdf_footer_skip: 0.0,
            ..ReflowOpts::default()
        };
        let mut log = ReflowLog::default();
        let doc = PdfDocument::from_xml(&xml, &mut opts, &mut log).expect("parses synthetic doc");
        let merged = doc.pages[0]
            .texts
            .iter()
            .find(|t| t.text_as_string.contains("This paragraph starts here"))
            .expect("first paragraph fragment survives");
        // The wrapped continuation line should have been folded into the
        // same Text as the line that started the paragraph.
        assert!(
            merged
                .text_as_string
                .contains("concludes on this second physical line"),
            "expected wrapped line to merge into the paragraph, got: {:?}",
            merged.text_as_string
        );
        // But the next, geometrically-separated paragraph should remain
        // its own Text.
        let next_para = doc.pages[0]
            .texts
            .iter()
            .find(|t| t.text_as_string.contains("A separate new paragraph"))
            .expect("second paragraph survives separately");
        assert!(!next_para
            .text_as_string
            .contains("concludes on this second"));
    }

    #[test]
    fn pdf_document_render_html_produces_well_formed_wrapper() {
        let xml = synthetic_doc_xml();
        let mut opts = ReflowOpts {
            pdf_header_skip: 0.0,
            pdf_footer_skip: 0.0,
            ..ReflowOpts::default()
        };
        let mut log = ReflowLog::default();
        let mut doc =
            PdfDocument::from_xml(&xml, &mut opts, &mut log).expect("parses synthetic doc");
        let html = doc.render_html("Test Book");
        assert!(html.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(html.contains("<title>Test Book</title>"));
        assert!(html.contains("Chapter One"));
        assert!(html.ends_with("</html>"));
    }

    #[test]
    fn pdf_document_write_html_writes_to_explicit_directory() {
        let xml = synthetic_doc_xml();
        let mut opts = ReflowOpts {
            pdf_header_skip: 0.0,
            pdf_footer_skip: 0.0,
            ..ReflowOpts::default()
        };
        let mut log = ReflowLog::default();
        let mut doc =
            PdfDocument::from_xml(&xml, &mut opts, &mut log).expect("parses synthetic doc");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = doc
            .write_html(dir.path(), "Test Book")
            .expect("writes index.html");
        assert_eq!(path, dir.path().join("index.html"));
        let contents = std::fs::read_to_string(&path).expect("reads back");
        assert!(contents.contains("Chapter One"));
    }

    #[test]
    fn pdf_document_explicit_header_skip_removes_matching_lines() {
        // Three pages that each repeat the same top-of-page text: with
        // opts.pdf_header_skip left negative (auto-detect) this should get
        // recognized as a running header and stripped from every page.
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<pdf2xml>
<page number="1" position="absolute" top="0" left="0" height="792" width="612">
{fonts}
<text top="20" left="72" width="200" height="12" font="0">Running Header Text</text>
<text top="100" left="72" width="300" height="14" font="0">Body content for page one goes here.</text>
</page>
<page number="2" position="absolute" top="0" left="0" height="792" width="612">
{fonts}
<text top="20" left="72" width="200" height="12" font="0">Running Header Text</text>
<text top="100" left="72" width="300" height="14" font="0">Body content for page two goes here.</text>
</page>
<page number="3" position="absolute" top="0" left="0" height="792" width="612">
{fonts}
<text top="20" left="72" width="200" height="12" font="0">Running Header Text</text>
<text top="100" left="72" width="300" height="14" font="0">Body content for page three goes here.</text>
</page>
</pdf2xml>"#,
            fonts = synthetic_fontspecs()
        );
        let mut opts = ReflowOpts::default(); // pdf_header_skip = -1.0 => auto-detect
        let mut log = ReflowLog::default();
        let doc = PdfDocument::from_xml(&xml, &mut opts, &mut log).expect("parses synthetic doc");
        let still_has_header = doc.pages.iter().any(|p| {
            p.texts
                .iter()
                .any(|t| t.text_as_string.contains("Running Header Text"))
        });
        assert!(
            !still_has_header,
            "auto-detected running header should have been removed"
        );
        // Body content on every surviving page should remain untouched.
        assert!(doc.pages.iter().any(|p| p
            .texts
            .iter()
            .any(|t| t.text_as_string.contains("Body content"))));
    }

    #[test]
    fn pdf_document_user_regex_removes_header_and_footer() {
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<pdf2xml>
<page number="1" position="absolute" top="0" left="0" height="792" width="612">
{fonts}
<text top="20" left="72" width="200" height="12" font="0">DRAFT COPY</text>
<text top="100" left="72" width="300" height="14" font="0">Body content goes here for the page.</text>
<text top="750" left="72" width="200" height="12" font="0">Page 1 of 1</text>
</page>
</pdf2xml>"#,
            fonts = synthetic_fontspecs()
        );
        let mut opts = ReflowOpts {
            pdf_header_skip: 0.0,
            pdf_footer_skip: 0.0,
            pdf_header_regex: r"^DRAFT".to_string(),
            pdf_footer_regex: r"^Page \d+ of".to_string(),
            ..ReflowOpts::default()
        };
        let mut log = ReflowLog::default();
        let doc = PdfDocument::from_xml(&xml, &mut opts, &mut log).expect("parses synthetic doc");
        let texts: Vec<&str> = doc.pages[0]
            .texts
            .iter()
            .map(|t| t.text_as_string.as_str())
            .collect();
        assert!(!texts.iter().any(|t| t.contains("DRAFT COPY")));
        assert!(!texts.iter().any(|t| t.contains("Page 1 of 1")));
        assert!(texts.iter().any(|t| t.contains("Body content")));
    }
}
