//! Paragraph-level formatting: reading `w:pPr` into a property set and
//! turning that set into CSS.
//!
//! Port of `old_src/src/calibre/ebooks/docx/block_styles.py`.
//!
//! # The `inherit` sentinel
//!
//! Word's style model is three-layered — document defaults, named
//! styles (themselves a `basedOn` chain), and direct formatting on the
//! element — and resolving it needs a third state beyond "true" and
//! "false": *unspecified at this layer, so take whatever the layer
//! above says*. Python models that with a singleton `inherit` object
//! stored in the same attribute as real values. Rust models it as
//! `None`: every property here is an `Option`, where `None` means
//! inherit and `Some(v)` means this layer sets it to `v`.
//!
//! That mapping is what makes [`ParagraphStyle::update`] and
//! [`ParagraphStyle::resolve_based_on`] one-liners per field instead of
//! the identity comparisons (`is not inherit`) the Python needs.

use indexmap::IndexMap;
use roxmltree::Node;

use super::names::DocxNamespace;

/// An ordered set of CSS declarations. Order is preserved because CSS
/// resolution depends on it — `margin-left` after `margin` must stay
/// after `margin`.
pub type Css = IndexMap<String, String>;

/// Format a number the way Python's `'{:.3g}'` does — three significant
/// digits, trailing zeros removed.
///
/// The CSS this module emits is compared byte-for-byte against
/// calibre's output, so the number formatting has to match rather than
/// merely be close.
pub fn format_g3(value: f64) -> String {
    format_g(value, 3)
}

fn format_g(value: f64, precision: usize) -> String {
    if !value.is_finite() {
        return format!("{value}");
    }
    if value == 0.0 {
        return "0".to_string();
    }
    let precision = precision.max(1);
    // Which exponent the value rounds to at this precision — computed
    // from the rounded string, since 9.9999 at 3 digits is 10.0, an
    // exponent higher than log10 alone would suggest.
    let exp = {
        let rounded: f64 = format!("{:.*e}", precision - 1, value)
            .parse()
            .unwrap_or(value);
        if rounded == 0.0 {
            0
        } else {
            rounded.abs().log10().floor() as i32
        }
    };

    if exp < -4 || exp >= precision as i32 {
        let s = format!("{:.*e}", precision - 1, value);
        // Rust renders the exponent as `e4`/`e-4`; Python as `e+04`.
        let (mantissa, e) = s.split_once('e').unwrap_or((s.as_str(), "0"));
        let mantissa = strip_trailing_zeros(mantissa);
        let exp_val: i32 = e.parse().unwrap_or(0);
        format!(
            "{mantissa}e{}{:02}",
            if exp_val < 0 { '-' } else { '+' },
            exp_val.abs()
        )
    } else {
        let decimals = (precision as i32 - 1 - exp).max(0) as usize;
        strip_trailing_zeros(&format!("{value:.decimals$}"))
    }
}

fn strip_trailing_zeros(s: &str) -> String {
    if !s.contains('.') {
        return s.to_string();
    }
    let trimmed = s.trim_end_matches('0');
    trimmed.trim_end_matches('.').to_string()
}

/// A length in points, rendered as CSS.
fn pt(value: f64) -> String {
    format!("{}pt", format_g3(value))
}

/// Word's border art styles mapped onto CSS `border-style` keywords.
///
/// Port of the Python `LINE_STYLES`.
pub fn line_style(val: &str) -> &'static str {
    match val {
        "basicBlackDashes" | "basicBlackSquares" | "dashed" | "dashSmallGap" | "dotDash"
        | "dotDotDash" => "dashed",
        "basicBlackDots" | "dotted" => "dotted",
        "basicThinLines" | "single" | "thick" => "solid",
        "dashDotStroked" | "threeDEngrave" => "groove",
        "double"
        | "thickThinLargeGap"
        | "thickThinMediumGap"
        | "thickThinSmallGap"
        | "thinThickLargeGap"
        | "thinThickMediumGap"
        | "thinThickSmallGap"
        | "thinThickThinLargeGap"
        | "thinThickThinMediumGap"
        | "thinThickThinSmallGap"
        | "triple" => "double",
        "inset" => "inset",
        "nil" | "none" => "none",
        "outset" => "outset",
        "threeDEmboss" => "ridge",
        // Anything unrecognised still draws a line.
        _ => "solid",
    }
}

/// Read a `w:*` on/off property: present means on unless `w:val` says
/// otherwise; absent means inherit.
///
/// Port of the Python `binary_property`.
pub fn binary_property(parent: Node, name: &str, ns: &DocxNamespace) -> Option<bool> {
    let elem = ns.first_child(parent, &format!("w:{name}"))?;
    let val = ns.get_or(elem, "w:val", "on");
    Some(matches!(val, "on" | "1" | "true"))
}

/// Turn a six-digit hex colour into a CSS colour. `auto` and malformed
/// values fall back to `auto_value`.
///
/// Port of the Python `simple_color`.
pub fn simple_color(col: Option<&str>, auto_value: &str) -> String {
    match col {
        Some(c) if c != "auto" && c.len() == 6 => format!("#{c}"),
        _ => auto_value.to_string(),
    }
}

/// Parse a number and scale it, yielding `None` on anything unparseable.
///
/// Port of the Python `simple_float`.
pub fn simple_float(val: Option<&str>, mult: f64) -> Option<f64> {
    Some(val?.trim().parse::<f64>().ok()? * mult)
}

/// Parse a length given either as twentieths of a point or as a number
/// with a `pt` suffix.
///
/// Port of the Python `twips`.
pub fn twips(val: Option<&str>, mult: f64) -> Option<f64> {
    let raw = val?;
    if let Some(v) = simple_float(Some(raw), mult) {
        return Some(v);
    }
    // Only the default (twentieths) scale accepts an explicit `pt`.
    if mult == 0.05 {
        if let Some(stripped) = raw.strip_suffix("pt") {
            return simple_float(Some(stripped), 1.0);
        }
    }
    None
}

/// The five border edges Word tracks. `Between` is the border drawn
/// between consecutive paragraphs sharing a style, which has no CSS
/// equivalent and is resolved into `Bottom` during conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Edge {
    Left,
    Top,
    Right,
    Bottom,
    Between,
}

impl Edge {
    /// The four edges that map directly onto CSS.
    pub const CSS_EDGES: [Edge; 4] = [Edge::Left, Edge::Top, Edge::Right, Edge::Bottom];
    /// Every edge, in the order the Python `border_edges` lists them.
    pub const ALL: [Edge; 5] = [
        Edge::Left,
        Edge::Top,
        Edge::Right,
        Edge::Bottom,
        Edge::Between,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Edge::Left => "left",
            Edge::Top => "top",
            Edge::Right => "right",
            Edge::Bottom => "bottom",
            Edge::Between => "between",
        }
    }
}

/// One edge's border properties. `None` means inherit.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Border {
    pub padding: Option<f64>,
    pub width: Option<f64>,
    pub style: Option<String>,
    pub color: Option<String>,
}

impl Border {
    fn update(&mut self, other: &Border) {
        if other.padding.is_some() {
            self.padding = other.padding;
        }
        if other.width.is_some() {
            self.width = other.width;
        }
        if other.style.is_some() {
            self.style.clone_from(&other.style);
        }
        if other.color.is_some() {
            self.color.clone_from(&other.color);
        }
    }

    fn resolve_based_on(&mut self, parent: &Border) {
        if self.padding.is_none() {
            self.padding = parent.padding;
        }
        if self.width.is_none() {
            self.width = parent.width;
        }
        if self.style.is_none() {
            self.style.clone_from(&parent.style);
        }
        if self.color.is_none() {
            self.color.clone_from(&parent.color);
        }
    }
}

/// All five edges of one element.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Borders {
    pub left: Border,
    pub top: Border,
    pub right: Border,
    pub bottom: Border,
    pub between: Border,
}

impl Borders {
    pub fn edge(&self, edge: Edge) -> &Border {
        match edge {
            Edge::Left => &self.left,
            Edge::Top => &self.top,
            Edge::Right => &self.right,
            Edge::Bottom => &self.bottom,
            Edge::Between => &self.between,
        }
    }

    pub fn edge_mut(&mut self, edge: Edge) -> &mut Border {
        match edge {
            Edge::Left => &mut self.left,
            Edge::Top => &mut self.top,
            Edge::Right => &mut self.right,
            Edge::Bottom => &mut self.bottom,
            Edge::Between => &mut self.between,
        }
    }

    fn update(&mut self, other: &Borders) {
        for edge in Edge::ALL {
            let src = other.edge(edge).clone();
            self.edge_mut(edge).update(&src);
        }
    }

    fn resolve_based_on(&mut self, parent: &Borders) {
        for edge in Edge::ALL {
            let src = parent.edge(edge).clone();
            self.edge_mut(edge).resolve_based_on(&src);
        }
    }
}

/// Read one `w:left`/`w:top`/... child of a border container.
///
/// Port of the Python `read_single_border`.
pub fn read_single_border(parent: Node, edge: Edge, ns: &DocxNamespace) -> Border {
    let mut border = Border::default();
    for elem in ns.children(parent, &[&format!("w:{}", edge.as_str())]) {
        if let Some(c) = ns.get(elem, "w:color") {
            border.color = Some(simple_color(Some(c), "currentColor"));
        }
        if let Some(s) = ns.get(elem, "w:val") {
            border.style = Some(line_style(s).to_string());
        }
        if let Some(space) = ns.get(elem, "w:space") {
            if let Ok(v) = space.trim().parse::<f64>() {
                border.padding = Some(v);
            }
        }
        if let Some(sz) = ns.get(elem, "w:sz") {
            // Art borders are only used for page borders, so the value
            // is always eighths of a point here.
            if let Ok(v) = sz.trim().parse::<f64>() {
                border.width = Some(v.clamp(2.0, 96.0) / 8.0);
            }
        }
    }
    border
}

/// Read a `w:pBdr` (or `w:tblBorders`, `w:tcBorders`, ...) container.
///
/// Port of the Python `read_border`.
pub fn read_border(parent: Node, ns: &DocxNamespace, name: &str, edges: &[Edge]) -> Borders {
    let mut borders = Borders::default();
    for container in ns.children(parent, &[&format!("w:{name}")]) {
        for &edge in edges {
            let read = read_single_border(container, edge, ns);
            borders.edge_mut(edge).update(&read);
        }
    }
    borders
}

/// Emit one edge's border declarations.
///
/// Port of the Python `border_to_css`.
pub fn border_to_css(edge: Edge, borders: &Borders, css: &mut Css) {
    let border = borders.edge(edge);
    let name = edge.as_str();
    if let Some(style) = &border.style {
        css.insert(format!("border-{name}-style"), style.clone());
    }
    if let Some(color) = &border.color {
        css.insert(format!("border-{name}-color"), color.clone());
    }
    if let Some(width) = border.width {
        // WebKit needs at least 1pt to render a border and 3pt to
        // render a double one.
        let floor = if border.style.as_deref() == Some("double") {
            3.0
        } else {
            1.0
        };
        css.insert(format!("border-{name}-width"), pt(width.max(floor)));
    }
}

/// Left/right margins and first-line indent, read from `w:ind`.
///
/// Port of the Python `read_indent`.
fn read_indent(parent: Node, ns: &DocxNamespace, dest: &mut ParagraphStyle) {
    for indent in ns.children(parent, &["w:ind"]) {
        // The `*Chars` variants are in hundredths of a character, so
        // they render as `em`; the plain ones are twips, so `pt`.
        let (l, lc) = (ns.get(indent, "w:left"), ns.get(indent, "w:leftChars"));
        if let Some(v) = char_or_twips(lc, l) {
            dest.margin_left = Some(v);
        }
        let (r, rc) = (ns.get(indent, "w:right"), ns.get(indent, "w:rightChars"));
        if let Some(v) = char_or_twips(rc, r) {
            dest.margin_right = Some(v);
        }

        // A hanging indent is a negative first-line indent, and takes
        // precedence over w:firstLine.
        let hanging = ns.get(indent, "w:hanging").map(|v| format!("-{v}"));
        let hanging_chars = ns.get(indent, "w:hangingChars").map(|v| format!("-{v}"));
        let first_line = ns.get(indent, "w:firstLine");
        let first_line_chars = ns.get(indent, "w:firstLineChars");

        let ti = char_or_twips(hanging_chars.as_deref(), hanging.as_deref())
            .or_else(|| char_or_twips(first_line_chars, first_line));
        if let Some(v) = ti {
            dest.text_indent = Some(v);
        }
    }
}

/// Prefer the hundredths-of-a-character value (rendered `em`) over the
/// twips one (rendered `pt`), matching the Python's nested conditional.
fn char_or_twips(chars: Option<&str>, twips_val: Option<&str>) -> Option<String> {
    if chars.is_some() {
        return simple_float(chars, 0.01).map(|v| format!("{}em", format_g3(v)));
    }
    simple_float(twips_val, 0.05).map(|v| pt(v))
}

/// Port of the Python `read_justification`.
fn read_justification(parent: Node, ns: &DocxNamespace, dest: &mut ParagraphStyle) {
    for jc in ns.children(parent, &["w:jc"]) {
        let Some(val) = ns.get(jc, "w:val").filter(|v| !v.is_empty()) else {
            continue;
        };
        // The `kashida` test is lowercase in calibre while Word writes
        // `lowKashida`/`mediumKashida`/`highKashida`, so it never
        // matches. Kept as-is: "fixing" it would change the rendering
        // of Arabic documents, which is not this port's call to make.
        if matches!(val, "both" | "distribute") || val.contains("thai") || val.contains("kashida") {
            dest.text_align = Some("justify".to_string());
        } else if matches!(val, "left" | "center" | "right" | "start" | "end") {
            dest.text_align = Some(val.to_string());
        }
    }
}

/// Port of the Python `read_spacing`.
fn read_spacing(parent: Node, ns: &DocxNamespace, dest: &mut ParagraphStyle) {
    for s in ns.children(parent, &["w:spacing"]) {
        // `*Lines` values are in hundredths of a line, rendered `ex`.
        // Autospacing means "let the renderer decide", i.e. emit
        // nothing at all.
        let after_auto = ns.get(s, "w:afterAutospacing");
        if !is_on(after_auto) {
            let lines = ns.get(s, "w:afterLines");
            let val = if lines.is_some() {
                simple_float(lines, 0.02).map(|v| format!("{}ex", format_g3(v)))
            } else {
                simple_float(ns.get(s, "w:after"), 0.05).map(pt)
            };
            if let Some(v) = val {
                dest.margin_bottom = Some(v);
            }
        }

        let before_auto = ns.get(s, "w:beforeAutospacing");
        if !is_on(before_auto) {
            let lines = ns.get(s, "w:beforeLines");
            let val = if lines.is_some() {
                simple_float(lines, 0.02).map(|v| format!("{}ex", format_g3(v)))
            } else {
                simple_float(ns.get(s, "w:before"), 0.05).map(pt)
            };
            if let Some(v) = val {
                dest.margin_top = Some(v);
            }
        }

        if let Some(line) = ns.get(s, "w:line") {
            // `exact`/`atLeast` are absolute (twips); `auto` is a
            // multiple of single spacing, where 240 means 1.0.
            let rule = ns.get_or(s, "w:lineRule", "auto");
            let absolute = matches!(rule, "exact" | "atLeast");
            let mult = if absolute { 0.05 } else { 1.0 / 240.0 };
            if let Some(lh) = simple_float(Some(line), mult) {
                dest.line_height = Some(if absolute { pt(lh) } else { format_g3(lh) });
            }
        }
    }
}

fn is_on(val: Option<&str>) -> bool {
    matches!(val, Some("on") | Some("1") | Some("true"))
}

/// Port of the Python `read_shd`. Shared with run-level formatting.
pub fn read_shd(parent: Node, ns: &DocxNamespace) -> Option<String> {
    let mut ans = None;
    for shd in ns.children(parent, &["w:shd"]) {
        if let Some(val) = ns.get(shd, "w:fill").filter(|v| !v.is_empty()) {
            ans = Some(simple_color(Some(val), "transparent"));
        }
    }
    ans
}

/// Port of the Python `read_numbering`.
fn read_numbering(parent: Node, ns: &DocxNamespace, dest: &mut ParagraphStyle) {
    for np in ns.children(parent, &["w:numPr"]) {
        for ilvl in ns.children(np, &["w:ilvl"]) {
            if let Some(v) = ns
                .get(ilvl, "w:val")
                .and_then(|v| v.trim().parse::<i32>().ok())
            {
                dest.numbering_level = Some(v);
            }
        }
        for num in ns.children(np, &["w:numId"]) {
            if let Some(v) = ns.get(num, "w:val") {
                dest.numbering_id = Some(v.to_string());
            }
        }
    }
}

/// A text frame — Word's floating-paragraph mechanism, also how drop
/// caps are expressed.
///
/// Port of the Python `Frame`.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub drop_cap: String,
    pub h: f64,
    pub w: Option<f64>,
    pub h_anchor: String,
    pub h_rule: String,
    pub v_anchor: String,
    pub wrap: String,
    pub h_space: f64,
    pub v_space: f64,
    pub lines: i32,
    pub x_align: Option<String>,
    pub y_align: Option<String>,
    pub x: f64,
    pub y: f64,
}

impl Frame {
    /// Read a `w:framePr` element.
    pub fn new(fp: Node, ns: &DocxNamespace) -> Self {
        let twentieths = |attr: &str, default: f64| -> f64 {
            ns.get(fp, attr)
                .and_then(|v| v.trim().parse::<i64>().ok())
                .map(|v| v as f64 / 20.0)
                .unwrap_or(default)
        };
        Self {
            drop_cap: ns.get_or(fp, "w:dropCap", "none").to_string(),
            h: twentieths("w:h", 0.0),
            w: ns
                .get(fp, "w:w")
                .and_then(|v| v.trim().parse::<i64>().ok())
                .map(|v| v as f64 / 20.0),
            x: twentieths("w:x", 0.0),
            y: twentieths("w:y", 0.0),
            h_anchor: ns.get_or(fp, "w:hAnchor", "page").to_string(),
            h_rule: ns.get_or(fp, "w:hRule", "auto").to_string(),
            v_anchor: ns.get_or(fp, "w:vAnchor", "page").to_string(),
            wrap: ns.get_or(fp, "w:wrap", "around").to_string(),
            x_align: ns.get(fp, "w:xAlign").map(str::to_string),
            y_align: ns.get(fp, "w:yAlign").map(str::to_string),
            h_space: twentieths("w:hSpace", 0.0),
            v_space: twentieths("w:vSpace", 0.0),
            lines: ns
                .get(fp, "w:lines")
                .and_then(|v| v.trim().parse::<i32>().ok())
                .unwrap_or(1),
        }
    }

    /// The CSS for this frame. `page_width` is the printable page width
    /// in points, used to decide which side an unaligned frame floats
    /// to.
    ///
    /// Port of the Python `Frame.css`.
    pub fn css(&self, page_width: f64) -> Css {
        let mut ans = Css::new();
        ans.insert("overflow".to_string(), "hidden".to_string());

        if matches!(self.drop_cap.as_str(), "drop" | "margin") {
            ans.insert("float".to_string(), "left".to_string());
            ans.insert("margin".to_string(), "0".to_string());
            ans.insert("padding-right".to_string(), "0.2em".to_string());
            return ans;
        }

        if self.h_rule != "auto" {
            let key = if self.h_rule == "atLeast" {
                "min-height"
            } else {
                "height"
            };
            ans.insert(key.to_string(), pt(self.h));
        }
        if let Some(w) = self.w {
            ans.insert("width".to_string(), pt(w));
        }
        ans.insert("padding-top".to_string(), pt(self.v_space));
        ans.insert("padding-bottom".to_string(), pt(self.v_space));
        if self.wrap != "none" {
            ans.insert("padding-left".to_string(), pt(self.h_space));
            ans.insert("padding-right".to_string(), pt(self.h_space));
            let float = match &self.x_align {
                Some(a) if a == "right" => "right",
                Some(_) => "left",
                None => {
                    if page_width != 0.0 && self.x / page_width >= 0.5 {
                        "right"
                    } else {
                        "left"
                    }
                }
            };
            ans.insert("float".to_string(), float.to_string());
        }
        ans
    }
}

/// Word's on/off paragraph properties, in the order the Python lists
/// them.
pub const BINARY_PROPERTIES: [&str; 15] = [
    "adjustRightInd",
    "autoSpaceDE",
    "autoSpaceDN",
    "bidi",
    "contextualSpacing",
    "keepLines",
    "keepNext",
    "mirrorIndents",
    "pageBreakBefore",
    "snapToGrid",
    "suppressLineNumbers",
    "suppressOverlap",
    "topLinePunct",
    "widowControl",
    "wordWrap",
];

/// The resolved (or partially resolved) formatting of one paragraph.
///
/// Port of the Python `ParagraphStyle`. Every field is `Option`, where
/// `None` is the Python `inherit`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParagraphStyle {
    // On/off properties, in `BINARY_PROPERTIES` order.
    pub adjust_right_ind: Option<bool>,
    pub auto_space_de: Option<bool>,
    pub auto_space_dn: Option<bool>,
    pub bidi: Option<bool>,
    pub contextual_spacing: Option<bool>,
    pub keep_lines: Option<bool>,
    pub keep_next: Option<bool>,
    pub mirror_indents: Option<bool>,
    pub page_break_before: Option<bool>,
    pub snap_to_grid: Option<bool>,
    pub suppress_line_numbers: Option<bool>,
    pub suppress_overlap: Option<bool>,
    pub top_line_punct: Option<bool>,
    pub widow_control: Option<bool>,
    pub word_wrap: Option<bool>,

    pub borders: Borders,
    pub margin_left: Option<String>,
    pub margin_top: Option<String>,
    pub margin_right: Option<String>,
    pub margin_bottom: Option<String>,

    pub text_indent: Option<String>,
    pub text_align: Option<String>,
    pub line_height: Option<String>,
    pub background_color: Option<String>,
    pub numbering_id: Option<String>,
    pub numbering_level: Option<i32>,
    pub font_family: Option<String>,
    pub font_size: Option<f64>,
    pub color: Option<String>,
    pub frame: Option<Frame>,
    pub cs_font_size: Option<f64>,
    pub cs_font_family: Option<String>,

    /// The `w:pStyle` this paragraph links to, if any. Not an
    /// inheritable property — it names where to inherit *from*.
    pub linked_style: Option<String>,
    /// The human-readable name of the linked style, filled in during
    /// style resolution.
    pub style_name: Option<String>,
}

impl ParagraphStyle {
    /// An all-inherit style, the Python `ParagraphStyle(namespace)`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Read a `w:pPr` element.
    ///
    /// Port of the Python `ParagraphStyle(namespace, pPr)`.
    pub fn from_ppr(ppr: Node, ns: &DocxNamespace) -> Self {
        let mut style = Self::new();
        style.adjust_right_ind = binary_property(ppr, "adjustRightInd", ns);
        style.auto_space_de = binary_property(ppr, "autoSpaceDE", ns);
        style.auto_space_dn = binary_property(ppr, "autoSpaceDN", ns);
        style.bidi = binary_property(ppr, "bidi", ns);
        style.contextual_spacing = binary_property(ppr, "contextualSpacing", ns);
        style.keep_lines = binary_property(ppr, "keepLines", ns);
        style.keep_next = binary_property(ppr, "keepNext", ns);
        style.mirror_indents = binary_property(ppr, "mirrorIndents", ns);
        style.page_break_before = binary_property(ppr, "pageBreakBefore", ns);
        style.snap_to_grid = binary_property(ppr, "snapToGrid", ns);
        style.suppress_line_numbers = binary_property(ppr, "suppressLineNumbers", ns);
        style.suppress_overlap = binary_property(ppr, "suppressOverlap", ns);
        style.top_line_punct = binary_property(ppr, "topLinePunct", ns);
        style.widow_control = binary_property(ppr, "widowControl", ns);
        style.word_wrap = binary_property(ppr, "wordWrap", ns);

        style.borders = read_border(ppr, ns, "pBdr", &Edge::ALL);
        read_indent(ppr, ns, &mut style);
        read_justification(ppr, ns, &mut style);
        read_spacing(ppr, ns, &mut style);
        style.background_color = read_shd(ppr, ns);
        read_numbering(ppr, ns, &mut style);
        for fp in ns.children(ppr, &["w:framePr"]) {
            style.frame = Some(Frame::new(fp, ns));
        }

        for s in ns.children(ppr, &["w:pStyle"]) {
            if let Some(val) = ns.get(s, "w:val") {
                style.linked_style = Some(val.to_string());
            }
        }
        style
    }

    /// Overlay `other`'s specified properties onto this one.
    ///
    /// Port of the Python `update`.
    pub fn update(&mut self, other: &ParagraphStyle) {
        macro_rules! overlay {
            ($($field:ident),* $(,)?) => {
                $(if other.$field.is_some() { self.$field.clone_from(&other.$field); })*
            };
        }
        overlay!(
            adjust_right_ind,
            auto_space_de,
            auto_space_dn,
            bidi,
            contextual_spacing,
            keep_lines,
            keep_next,
            mirror_indents,
            page_break_before,
            snap_to_grid,
            suppress_line_numbers,
            suppress_overlap,
            top_line_punct,
            widow_control,
            word_wrap,
            margin_left,
            margin_top,
            margin_right,
            margin_bottom,
            text_indent,
            text_align,
            line_height,
            background_color,
            numbering_id,
            numbering_level,
            font_family,
            font_size,
            color,
            frame,
            cs_font_size,
            cs_font_family,
        );
        self.borders.update(&other.borders);
        if other.linked_style.is_some() {
            self.linked_style.clone_from(&other.linked_style);
        }
    }

    /// Fill every inherited property from `parent`.
    ///
    /// Port of the Python `resolve_based_on`.
    pub fn resolve_based_on(&mut self, parent: &ParagraphStyle) {
        macro_rules! inherit {
            ($($field:ident),* $(,)?) => {
                $(if self.$field.is_none() { self.$field.clone_from(&parent.$field); })*
            };
        }
        inherit!(
            adjust_right_ind,
            auto_space_de,
            auto_space_dn,
            bidi,
            contextual_spacing,
            keep_lines,
            keep_next,
            mirror_indents,
            page_break_before,
            snap_to_grid,
            suppress_line_numbers,
            suppress_overlap,
            top_line_punct,
            widow_control,
            word_wrap,
            margin_left,
            margin_top,
            margin_right,
            margin_bottom,
            text_indent,
            text_align,
            line_height,
            background_color,
            numbering_id,
            numbering_level,
            font_family,
            font_size,
            color,
            frame,
            cs_font_size,
            cs_font_family,
        );
        self.borders.resolve_based_on(&parent.borders);
    }

    /// The CSS for this paragraph.
    ///
    /// Port of the Python `ParagraphStyle.css`.
    pub fn css(&self) -> Css {
        let mut c = Css::new();
        if self.keep_lines == Some(true) {
            c.insert("page-break-inside".to_string(), "avoid".to_string());
        }
        if self.page_break_before == Some(true) {
            c.insert("page-break-before".to_string(), "always".to_string());
        }
        if self.keep_next == Some(true) {
            c.insert("page-break-after".to_string(), "avoid".to_string());
        }
        for edge in Edge::CSS_EDGES {
            border_to_css(edge, &self.borders, &mut c);
            let name = edge.as_str();
            if let Some(padding) = self.borders.edge(edge).padding {
                c.insert(format!("padding-{name}"), pt(padding));
            }
            if let Some(margin) = self.margin(edge) {
                c.insert(format!("margin-{name}"), margin.clone());
            }
        }

        // A line-height of exactly single spacing is Word's default and
        // carries no information.
        if let Some(lh) = self.line_height.as_deref().filter(|v| *v != "1") {
            c.insert("line-height".to_string(), lh.to_string());
        }

        if let Some(v) = &self.text_indent {
            c.insert("text-indent".to_string(), v.clone());
        }
        if let Some(v) = &self.background_color {
            c.insert("background-color".to_string(), v.clone());
        }
        if let Some(v) = &self.font_family {
            c.insert("font-family".to_string(), v.clone());
        }
        if let Some(v) = self.font_size {
            c.insert("font-size".to_string(), pt(v));
        }
        if let Some(v) = &self.color {
            c.insert("color".to_string(), v.clone());
        }

        if let Some(ta) = &self.text_align {
            // In a right-to-left paragraph Word's left/right mean the
            // opposite of CSS's.
            let ta = if self.bidi == Some(true) {
                match ta.as_str() {
                    "left" => "right",
                    "right" => "left",
                    other => other,
                }
            } else {
                ta.as_str()
            };
            c.insert("text-align".to_string(), ta.to_string());
        }
        c
    }

    fn margin(&self, edge: Edge) -> Option<&String> {
        match edge {
            Edge::Left => self.margin_left.as_ref(),
            Edge::Top => self.margin_top.as_ref(),
            Edge::Right => self.margin_right.as_ref(),
            Edge::Bottom => self.margin_bottom.as_ref(),
            Edge::Between => None,
        }
    }

    /// Whether two paragraphs' borders are identical — the test for
    /// merging consecutive bordered paragraphs into one block.
    ///
    /// Port of the Python `has_identical_borders`.
    pub fn has_identical_borders(&self, other: &ParagraphStyle) -> bool {
        self.borders == other.borders
    }

    /// Drop every CSS-visible border, leaving `between` alone.
    ///
    /// Port of the Python `clear_borders`.
    pub fn clear_borders(&mut self) {
        for edge in Edge::CSS_EDGES {
            let b = self.borders.edge_mut(edge);
            b.width = None;
            b.color = None;
            b.style = None;
        }
    }

    /// A bare style carrying only this one's CSS-visible borders.
    ///
    /// Port of the Python `clone_border_styles`.
    pub fn clone_border_styles(&self) -> ParagraphStyle {
        let mut style = ParagraphStyle::new();
        for edge in Edge::CSS_EDGES {
            let src = self.borders.edge(edge);
            let dest = style.borders.edge_mut(edge);
            dest.width = src.width;
            dest.color.clone_from(&src.color);
            dest.style.clone_from(&src.style);
        }
        style
    }

    /// Promote the `between` border to the bottom edge, which is how a
    /// between-paragraph rule is rendered in CSS.
    ///
    /// Port of the Python `apply_between_border`.
    pub fn apply_between_border(&mut self) {
        let between = self.borders.between.clone();
        self.borders.bottom.width = between.width;
        self.borders.bottom.color = between.color;
        self.borders.bottom.style = between.style;
    }

    /// Whether any CSS-visible edge actually draws.
    ///
    /// Port of the Python `has_visible_border`.
    pub fn has_visible_border(&self) -> bool {
        Edge::CSS_EDGES.iter().any(|&edge| {
            let b = self.borders.edge(edge);
            matches!(b.width, Some(w) if w != 0.0)
                && matches!(b.style.as_deref(), Some(s) if s != "none")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roxmltree::Document;

    const DOC_OPEN: &str =
        r#"<w:pPr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#;

    fn ppr(body: &str) -> String {
        format!("{DOC_OPEN}{body}</w:pPr>")
    }

    fn parse(body: &str) -> (Document<'static>, DocxNamespace) {
        let xml: &'static str = Box::leak(ppr(body).into_boxed_str());
        (
            Document::parse(xml).expect("valid XML"),
            DocxNamespace::default(),
        )
    }

    fn style_of(body: &str) -> ParagraphStyle {
        let (doc, ns) = parse(body);
        ParagraphStyle::from_ppr(doc.root_element(), &ns)
    }

    /// Values checked against Python's `'{:.3g}'.format(v)`.
    #[test]
    fn number_formatting_matches_python_g3() {
        for (value, expected) in [
            (0.0, "0"),
            (1.0, "1"),
            (1.5, "1.5"),
            (12.0, "12"),
            (12.5, "12.5"),
            (0.5, "0.5"),
            (1.25, "1.25"),
            (1.005, "1"),
            (123.456, "123"),
            (1234.5, "1.23e+03"),
            (0.000123, "0.000123"),
            (0.0000123, "1.23e-05"),
            (9.9999, "10"),
            (-12.5, "-12.5"),
            (0.05, "0.05"),
            (36.0, "36"),
        ] {
            assert_eq!(format_g3(value), expected, "formatting {value}");
        }
    }

    #[test]
    fn binary_properties_distinguish_off_from_unset() {
        let s = style_of(r#"<w:keepNext/><w:keepLines w:val="off"/>"#);
        assert_eq!(s.keep_next, Some(true), "a bare element means on");
        assert_eq!(s.keep_lines, Some(false), "an explicit off means off");
        assert_eq!(s.widow_control, None, "an absent element means inherit");
    }

    #[test]
    fn twips_accepts_both_spellings() {
        assert_eq!(twips(Some("240"), 0.05), Some(12.0));
        assert_eq!(twips(Some("12pt"), 0.05), Some(12.0));
        assert_eq!(twips(Some("junk"), 0.05), None);
        assert_eq!(twips(None, 0.05), None);
        // The `pt` suffix is only honoured at the default scale.
        assert_eq!(twips(Some("12pt"), 1.0), None);
    }

    #[test]
    fn colors_fall_back_when_auto_or_malformed() {
        assert_eq!(simple_color(Some("ff0000"), "currentColor"), "#ff0000");
        assert_eq!(simple_color(Some("auto"), "currentColor"), "currentColor");
        assert_eq!(simple_color(Some("f00"), "transparent"), "transparent");
        assert_eq!(simple_color(None, "currentColor"), "currentColor");
    }

    #[test]
    fn indents_render_twips_as_points_and_chars_as_em() {
        let s = style_of(r#"<w:ind w:left="720" w:right="360" w:firstLine="240"/>"#);
        assert_eq!(s.margin_left.as_deref(), Some("36pt"));
        assert_eq!(s.margin_right.as_deref(), Some("18pt"));
        assert_eq!(s.text_indent.as_deref(), Some("12pt"));

        let s = style_of(r#"<w:ind w:leftChars="200" w:firstLineChars="100"/>"#);
        assert_eq!(s.margin_left.as_deref(), Some("2em"));
        assert_eq!(s.text_indent.as_deref(), Some("1em"));
    }

    #[test]
    fn a_hanging_indent_is_negative_and_beats_first_line() {
        let s = style_of(r#"<w:ind w:hanging="360" w:firstLine="720"/>"#);
        assert_eq!(s.text_indent.as_deref(), Some("-18pt"));
    }

    #[test]
    fn justification_maps_word_values_onto_css() {
        assert_eq!(
            style_of(r#"<w:jc w:val="both"/>"#).text_align.as_deref(),
            Some("justify")
        );
        assert_eq!(
            style_of(r#"<w:jc w:val="thaiDistribute"/>"#)
                .text_align
                .as_deref(),
            Some("justify")
        );
        // Word spells the kashida values `lowKashida`/`mediumKashida`/
        // `highKashida`, so calibre's lowercase `'kashida' in val` test
        // never fires. Reproduced rather than fixed: changing it would
        // silently alter output for Arabic documents, which belongs in
        // its own change.
        assert_eq!(
            style_of(r#"<w:jc w:val="mediumKashida"/>"#)
                .text_align
                .as_deref(),
            None
        );
        assert_eq!(
            style_of(r#"<w:jc w:val="center"/>"#).text_align.as_deref(),
            Some("center")
        );
        assert_eq!(
            style_of(r#"<w:jc w:val="nonsense"/>"#)
                .text_align
                .as_deref(),
            None
        );
    }

    #[test]
    fn spacing_handles_lines_points_and_autospacing() {
        let s = style_of(r#"<w:spacing w:before="240" w:after="120"/>"#);
        assert_eq!(s.margin_top.as_deref(), Some("12pt"));
        assert_eq!(s.margin_bottom.as_deref(), Some("6pt"));

        let s = style_of(r#"<w:spacing w:beforeLines="100" w:afterLines="50"/>"#);
        assert_eq!(s.margin_top.as_deref(), Some("2ex"));
        assert_eq!(s.margin_bottom.as_deref(), Some("1ex"));

        // Autospacing means "renderer's choice", so nothing is emitted
        // even though a value is present.
        let s = style_of(r#"<w:spacing w:before="240" w:beforeAutospacing="1"/>"#);
        assert_eq!(s.margin_top, None);
    }

    #[test]
    fn line_height_is_a_multiple_unless_the_rule_is_absolute() {
        let s = style_of(r#"<w:spacing w:line="480" w:lineRule="auto"/>"#);
        assert_eq!(
            s.line_height.as_deref(),
            Some("2"),
            "480/240 = double spaced"
        );

        let s = style_of(r#"<w:spacing w:line="240" w:lineRule="exact"/>"#);
        assert_eq!(s.line_height.as_deref(), Some("12pt"));

        // Single spacing carries no information and is dropped from CSS.
        let s = style_of(r#"<w:spacing w:line="240" w:lineRule="auto"/>"#);
        assert_eq!(s.line_height.as_deref(), Some("1"));
        assert!(!s.css().contains_key("line-height"));
    }

    #[test]
    fn borders_read_width_style_color_and_padding() {
        let s = style_of(
            r#"<w:pBdr><w:top w:val="single" w:sz="8" w:space="4" w:color="FF0000"/></w:pBdr>"#,
        );
        let top = &s.borders.top;
        assert_eq!(top.style.as_deref(), Some("solid"));
        assert_eq!(top.color.as_deref(), Some("#FF0000"));
        assert_eq!(top.width, Some(1.0), "8 eighths of a point");
        assert_eq!(top.padding, Some(4.0));
        assert!(s.has_visible_border());

        let css = s.css();
        assert_eq!(
            css.get("border-top-style").map(String::as_str),
            Some("solid")
        );
        assert_eq!(css.get("border-top-width").map(String::as_str), Some("1pt"));
        assert_eq!(css.get("padding-top").map(String::as_str), Some("4pt"));
    }

    #[test]
    fn border_width_is_floored_so_webkit_renders_it() {
        // A hairline border (2 eighths = 0.25pt) would vanish in
        // WebKit, and a double border needs 3pt to show two lines.
        let s = style_of(r#"<w:pBdr><w:top w:val="single" w:sz="2"/></w:pBdr>"#);
        assert_eq!(s.borders.top.width, Some(0.25));
        assert_eq!(
            s.css().get("border-top-width").map(String::as_str),
            Some("1pt")
        );

        let s = style_of(r#"<w:pBdr><w:top w:val="double" w:sz="8"/></w:pBdr>"#);
        assert_eq!(
            s.css().get("border-top-width").map(String::as_str),
            Some("3pt")
        );
    }

    #[test]
    fn oversized_border_widths_are_clamped() {
        let s = style_of(r#"<w:pBdr><w:top w:val="single" w:sz="400"/></w:pBdr>"#);
        assert_eq!(s.borders.top.width, Some(12.0), "96/8 is the ceiling");
    }

    #[test]
    fn a_none_style_is_not_a_visible_border() {
        let s = style_of(r#"<w:pBdr><w:top w:val="none" w:sz="8"/></w:pBdr>"#);
        assert!(!s.has_visible_border());
    }

    #[test]
    fn between_borders_promote_to_the_bottom_edge() {
        let mut s =
            style_of(r#"<w:pBdr><w:between w:val="single" w:sz="8" w:color="00FF00"/></w:pBdr>"#);
        assert!(!s.has_visible_border(), "between is not a CSS edge");
        s.apply_between_border();
        assert!(s.has_visible_border());
        assert_eq!(s.borders.bottom.color.as_deref(), Some("#00FF00"));
    }

    #[test]
    fn clearing_and_cloning_borders_round_trip() {
        let s = style_of(r#"<w:pBdr><w:left w:val="dotted" w:sz="16"/></w:pBdr>"#);
        let clone = s.clone_border_styles();
        assert!(clone.has_identical_borders(&s));

        let mut cleared = s.clone();
        cleared.clear_borders();
        assert!(!cleared.has_visible_border());
        assert!(!cleared.has_identical_borders(&s));
    }

    #[test]
    fn update_overlays_only_specified_properties() {
        let mut base = style_of(r#"<w:jc w:val="center"/><w:keepNext/>"#);
        let overlay = style_of(r#"<w:jc w:val="right"/>"#);
        base.update(&overlay);
        assert_eq!(base.text_align.as_deref(), Some("right"), "overlaid");
        assert_eq!(base.keep_next, Some(true), "untouched by the overlay");
    }

    #[test]
    fn resolve_based_on_fills_only_inherited_properties() {
        let mut child = style_of(r#"<w:jc w:val="right"/>"#);
        let parent = style_of(r#"<w:jc w:val="center"/><w:spacing w:before="240"/>"#);
        child.resolve_based_on(&parent);
        assert_eq!(child.text_align.as_deref(), Some("right"), "child wins");
        assert_eq!(child.margin_top.as_deref(), Some("12pt"), "inherited");
    }

    #[test]
    fn bidi_paragraphs_mirror_their_alignment() {
        let s = style_of(r#"<w:bidi/><w:jc w:val="left"/>"#);
        assert_eq!(s.css().get("text-align").map(String::as_str), Some("right"));
    }

    #[test]
    fn numbering_is_read_from_numpr() {
        let s = style_of(r#"<w:numPr><w:ilvl w:val="2"/><w:numId w:val="7"/></w:numPr>"#);
        assert_eq!(s.numbering_level, Some(2));
        assert_eq!(s.numbering_id.as_deref(), Some("7"));
    }

    #[test]
    fn page_break_properties_reach_the_css() {
        let s = style_of(r#"<w:pageBreakBefore/><w:keepNext/><w:keepLines/>"#);
        let css = s.css();
        assert_eq!(
            css.get("page-break-before").map(String::as_str),
            Some("always")
        );
        assert_eq!(
            css.get("page-break-after").map(String::as_str),
            Some("avoid")
        );
        assert_eq!(
            css.get("page-break-inside").map(String::as_str),
            Some("avoid")
        );
    }

    #[test]
    fn a_drop_cap_frame_floats_left_and_ignores_geometry() {
        let (doc, ns) = parse(r#"<w:framePr w:dropCap="drop" w:w="2000" w:h="500"/>"#);
        let fp = ns
            .children(doc.root_element(), &["w:framePr"])
            .into_iter()
            .next()
            .unwrap();
        let frame = Frame::new(fp, &ns);
        let css = frame.css(595.28);
        assert_eq!(css.get("float").map(String::as_str), Some("left"));
        assert_eq!(css.get("padding-right").map(String::as_str), Some("0.2em"));
        assert!(
            !css.contains_key("width"),
            "geometry is ignored for drop caps"
        );
    }

    #[test]
    fn an_unaligned_frame_floats_to_whichever_half_it_sits_in() {
        let (doc, ns) = parse(r#"<w:framePr w:x="12000" w:w="2000" w:hSpace="180"/>"#);
        let fp = ns.children(doc.root_element(), &["w:framePr"])[0];
        let frame = Frame::new(fp, &ns);
        // x = 600pt on a 595.28pt page is past the midpoint.
        assert_eq!(
            frame.css(595.28).get("float").map(String::as_str),
            Some("right")
        );

        let (doc, ns) = parse(r#"<w:framePr w:x="1000" w:w="2000"/>"#);
        let fp = ns.children(doc.root_element(), &["w:framePr"])[0];
        assert_eq!(
            Frame::new(fp, &ns)
                .css(595.28)
                .get("float")
                .map(String::as_str),
            Some("left")
        );
    }

    #[test]
    fn shading_reads_the_fill_colour() {
        let s = style_of(r#"<w:shd w:fill="CCCCCC"/>"#);
        assert_eq!(s.background_color.as_deref(), Some("#CCCCCC"));
        let s = style_of(r#"<w:shd w:fill="auto"/>"#);
        assert_eq!(s.background_color.as_deref(), Some("transparent"));
    }

    #[test]
    fn an_empty_ppr_inherits_everything() {
        let s = style_of("");
        assert_eq!(s, ParagraphStyle::new());
        assert!(s.css().is_empty());
    }
}
