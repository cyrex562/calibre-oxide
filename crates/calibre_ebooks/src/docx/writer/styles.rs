//! CSS -> OOXML property conversion: `docx/writer/styles.py`'s
//! `TextStyle`/`BlockStyle` (data model + serialization) and
//! `FloatSpec` (data model only, so far).
//!
//! Port of `TextStyle`/`BlockStyle`/`FloatSpec`'s CSS-reading
//! constructors, plus `TextStyle`/`BlockStyle`'s `serialize`/
//! `serialize_properties` methods (writing to real `w:rPr`/`w:pPr` XML
//! via [`super::xml::Element`]), plus the module-level helpers
//! (`css_font_family_to_docx`/`parse_css_font_family`,
//! `convert_underline`, `bmap`, `LINE_STYLES`, `is_dropcaps`,
//! `read_css_block_borders`, `parse_css_length`). `FloatSpec.serialize`
//! (real `w:framePr` XML), `DOCXStyle`'s hash/dedup base class (`id`/
//! `name`/`next_style` bookkeeping), `CombinedStyle`,
//! `DescendantTextStyle`, and `StylesManager` (deduplication +
//! `w:styles` assembly) are not ported yet -- see issue #23's own
//! tracking notes. `serialize`'s `id`/`name`/`is_normal_style`/
//! `next_style` are plain parameters here rather than fields Python
//! stores on `self` (`DOCXStyle.id`/`.name`/`.next_style`), since
//! nothing yet needs to persist them on a `TextStyle`/`BlockStyle`
//! instance itself -- that's exactly the bookkeeping `StylesManager`'s
//! still-unported `finalize` assigns.
//!
//! Reads against [`crate::oeb::polish::style::Style`] -- the seam
//! issue #132 needed, not the stub `oeb::stylizer::Stylizer` its own
//! docs once assumed was still the state of things (see
//! `oeb/polish/style.rs`'s module docs for that correction).

use std::collections::HashSet;

use crate::dom::{Dom, NodeId};
use crate::oeb::polish::style::{ItemValue, Style, VerticalAlign};

use super::utils::{convert_color, int_or_zero};
use super::xml::Element;

/// The four box edges DOCX borders/padding/margins are read per, in
/// the order Python iterates them.
pub const BORDER_EDGES: [&str; 4] = ["left", "top", "right", "bottom"];

/// Port of `LINE_STYLES`: CSS `border-style` keyword -> OOXML
/// `w:val`.
pub fn line_style(css_style: &str) -> &'static str {
    match css_style {
        "none" | "hidden" => "none",
        "dotted" => "dotted",
        "dashed" => "dashed",
        "solid" => "single",
        "double" => "double",
        "groove" => "threeDEngrave",
        "ridge" => "threeDEmboss",
        "inset" => "inset",
        "outset" => "outset",
        _ => "none",
    }
}

/// Port of `bmap`: a Python bool -> OOXML's `on`/`off` toggle value.
pub fn bmap(x: bool) -> &'static str {
    if x {
        "on"
    } else {
        "off"
    }
}

/// Splits a `font-family` CSS value into its comma-separated
/// candidate names, unquoting each and stopping at a bare `inherit`
/// token -- port of `parse_css_font_family`.
///
/// Python tokenizes the value with a real CSS parser (`tinycss`) to
/// tell a quoted string token from a bare identifier; this does the
/// same job with a plain comma-split plus quote-trim, a disclosed
/// simplification (this crate has no standalone CSS *value* tokenizer
/// to port faithfully against -- [`crate::css`] parses whole
/// stylesheets/selectors, not one property value in isolation).
pub fn parse_css_font_family(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in raw.split(',') {
        let name = part.trim().trim_matches('"').trim_matches('\'').trim();
        if name.is_empty() {
            continue;
        }
        if name.eq_ignore_ascii_case("inherit") {
            break;
        }
        out.push(name.to_string());
    }
    out
}

/// Port of `css_font_family_to_docx`: the first usable CSS family
/// name, with a CSS generic family (`serif`, `sans-serif`, ...) mapped
/// to a real Word-installed font Word ships with.
pub fn css_font_family_to_docx(raw: &str) -> Option<String> {
    let first = parse_css_font_family(raw).into_iter().next()?;
    let generic = match first.to_ascii_lowercase().as_str() {
        "serif" => Some("Cambria"),
        "sansserif" | "sans-serif" => Some("Candara"),
        "fantasy" => Some("Comic Sans"),
        "cursive" => Some("Segoe Script"),
        _ => None,
    };
    Some(generic.map(str::to_string).unwrap_or(first))
}

/// Port of `convert_underline`: folds `text-decoration`'s
/// space-separated tokens (line style, line kind, colour) into one
/// `"<style> <color>"` string, or `""` when there's no underline.
/// `items` is `effective_text_decoration`'s value, already split on
/// whitespace by the caller (matching Python's own
/// `set((css.effective_text_decoration or '').split())`).
pub fn convert_underline(items: &HashSet<&str>) -> String {
    let mut style = "solid";
    let mut has_underline = false;
    let mut color = "auto".to_string();
    for &x in items {
        match x {
            "solid" | "double" | "dotted" | "dashed" | "wavy" => {
                style = match x {
                    "solid" => "single",
                    "wavy" => "wave",
                    "dashed" => "dash",
                    other => other,
                };
            }
            "underline" => has_underline = true,
            "overline" | "line-through" | "blink" => {}
            "none" => has_underline = false,
            other => {
                if let Some(c) = convert_color(Some(other)) {
                    color = c;
                }
            }
        }
    }
    if has_underline {
        format!("{style} {color}")
    } else {
        String::new()
    }
}

/// Reads one border edge's width, mapping the CSS keyword widths
/// (`thin`/`medium`/`thick`) to their pseudo-point sizes when the
/// resolved value isn't already a length -- port of the repeated
/// `if not isinstance(val, numbers.Number): val = {...}.get(val, 0)`
/// snippet shared by `TextStyle`/`read_css_block_borders`.
fn border_width_value(style: &Style, edge: &str) -> f64 {
    match style.item(&format!("border-{edge}-width")) {
        ItemValue::Number(n) => n,
        ItemValue::Text(t) => match t.as_str() {
            "thin" => 0.2,
            "medium" => 1.0,
            "thick" => 2.0,
            _ => 0.0,
        },
    }
}

/// Splits a CSS length into `(number, lowercased unit)` -- port of
/// `calibre.ebooks.parse_css_length`. `None` for `value: None`
/// (Python's `UNIT_RE.match(None)` raises `TypeError`, caught and
/// turned into `(None, None)`), an unparseable value, or one with no
/// number at all (Python's own falsy-empty-string check on
/// `m.group(1)`).
fn parse_css_length(value: Option<&str>) -> Option<(f64, String)> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"^(-*[0-9]*[.]?[0-9]*)\s*(%|em|ex|en|px|mm|cm|in|pt|pc|rem|q)$").unwrap()
    });
    let caps = re.captures(value?)?;
    let num_str = caps.get(1)?.as_str();
    if num_str.is_empty() {
        return None;
    }
    let num: f64 = num_str.parse().ok()?;
    Some((num, caps.get(2)?.as_str().to_ascii_lowercase()))
}

/// The run-level CSS -> `w:rPr` property set -- port of `TextStyle`'s
/// `ALL_PROPS` fields (everything `__init__` computes from `css`).
/// Deliberately excludes `DOCXStyle`'s bookkeeping fields (`id`/
/// `name`/`next_style`), which belong to `StylesManager`'s still-
/// unported deduplication pass -- see [`TextStyle::serialize`] for why
/// they're plain parameters there instead.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextStyle {
    pub font_family: Option<String>,
    /// Half-points (`w:sz`'s real unit), or `None` when unparseable.
    pub font_size: Option<i64>,
    pub bold: bool,
    pub italic: bool,
    pub color: Option<String>,
    pub background_color: Option<String>,
    /// `"<w:val> <w:color>"`, or empty for no underline -- port of
    /// `convert_underline`'s return, stored as-is (matches Python
    /// storing the same combined string on `self.underline`).
    pub underline: String,
    pub strike: bool,
    pub dstrike: bool,
    pub caps: bool,
    pub small_caps: bool,
    pub shadow: bool,
    /// Twentieths of a point, or `None` when unparseable.
    pub spacing: Option<i64>,
    /// `superscript`/`subscript`/`baseline`, or a raw point offset
    /// (as a decimal string in half-points) for `w:position`.
    pub vertical_align: String,
    pub padding: i64,
    pub border_width: i64,
    pub border_style: String,
    pub border_color: String,
}

impl TextStyle {
    /// Port of `TextStyle.__init__`. `is_parent_style` matches
    /// Python's own parameter: a run's *containing block*'s style is
    /// computed the same way but without `background_color`/borders
    /// (DOCX has no per-run background/border support at the
    /// containing-block level -- only the innermost run's own values
    /// apply, via [`DescendantTextStyle`], not ported yet).
    pub fn from_css(css: &Style, is_parent_style: bool) -> Self {
        let font_family = css_font_family_to_docx(&css.get("font-family"));

        let font_size = {
            let pts = css.font_size();
            if pts.is_finite() {
                Some((pts * 2.0).max(0.0) as i64)
            } else {
                None
            }
        };

        let fw = css.get("font-weight");
        let bold = matches!(fw.to_ascii_lowercase().as_str(), "bold" | "bolder")
            || int_or_zero(Some(&fw)) >= 700;
        let italic = matches!(
            css.get("font-style").to_ascii_lowercase().as_str(),
            "italic" | "oblique"
        );
        let color = convert_color(Some(&css.color()));
        let background_color = if is_parent_style {
            None
        } else {
            convert_color(css.background_color().as_deref())
        };

        let decoration = css.effective_text_decoration().unwrap_or_default();
        let td: HashSet<&str> = decoration.split_whitespace().collect();
        let underline = convert_underline(&td);
        let dstrike = td.contains("line-through") && td.contains("overline");
        let strike = !dstrike && td.contains("line-through");

        let text_transform = css.get("text-transform");
        let caps = text_transform == "uppercase";
        let small_caps = matches!(
            css.get("font-variant").to_ascii_lowercase().as_str(),
            "small-caps" | "smallcaps"
        );
        let text_shadow = css.get("text-shadow");
        let shadow = !matches!(text_shadow.as_str(), "none" | "");

        let spacing = match css.item("letter-spacing") {
            ItemValue::Number(n) => Some((n * 20.0) as i64),
            ItemValue::Text(_) => None,
        };

        let vertical_align = match css.first_vertical_align() {
            Some(VerticalAlign::Points(pts)) => ((pts * 2.0) as i64).to_string(),
            Some(VerticalAlign::Keyword(kw)) => match kw.as_str() {
                "top" | "text-top" | "sup" | "super" => "superscript".to_string(),
                "bottom" | "text-bottom" | "sub" => "subscript".to_string(),
                _ => "baseline".to_string(),
            },
            None => "baseline".to_string(),
        };

        // Ports Python's `self.padding = self.border_color =
        // self.border_width = self.border_style = None`, then per-edge
        // `if self.X is None: self.X = val elif self.X != val: self.X
        // = ignore`. `None` doubles as "not yet set" *and* a genuinely
        // valid value convert_color can return -- so an edge with no
        // colour, followed by an edge that DOES have one, silently
        // overwrites rather than triggering "mixed", exactly like
        // Python (only once a real value has been locked in does a
        // later mismatch flip the property to "mixed"/`ignore`).
        // `padding`/`border_width`/`border_style` never legitimately
        // hold `None`, so `first.is_none()` never misfires for them --
        // this generic accumulator is correct for all four fields.
        struct Acc<T> {
            value: Option<T>,
            mixed: bool,
        }
        impl<T: PartialEq> Acc<T> {
            fn new() -> Self {
                Acc {
                    value: None,
                    mixed: false,
                }
            }
            fn update(&mut self, v: Option<T>) {
                if self.mixed {
                    return;
                }
                match &self.value {
                    None => self.value = v,
                    Some(existing) if Some(existing) != v.as_ref() => self.mixed = true,
                    _ => {}
                }
            }
        }

        let mut padding_acc: Acc<i64> = Acc::new();
        let mut width_acc: Acc<i64> = Acc::new();
        let mut color_acc: Acc<String> = Acc::new();
        let mut style_acc: Acc<String> = Acc::new();

        if !is_parent_style {
            // DOCX does not support individual borders/padding for
            // inline content: fold every edge into one shared value.
            for edge in BORDER_EDGES {
                padding_acc.update(Some(padding_for(css, edge).max(0.0) as i64));

                let raw_w = border_width_value(css, edge);
                // Python's `int(val * 8)` truncates toward zero.
                let w = ((raw_w * 8.0) as i64).clamp(2, 96);
                width_acc.update(Some(w));

                let c = convert_color(Some(&css.get(&format!("border-{edge}-color"))));
                color_acc.update(c);

                let s = line_style(
                    &css.get(&format!("border-{edge}-style"))
                        .to_ascii_lowercase(),
                )
                .to_string();
                style_acc.update(Some(s));
            }
        }

        let padding = if padding_acc.mixed {
            0
        } else {
            padding_acc.value.unwrap_or(0)
        };
        let mut border_width = if width_acc.mixed {
            0
        } else {
            width_acc.value.unwrap_or(0)
        };
        let border_style = if style_acc.mixed {
            "none".to_string()
        } else {
            style_acc.value.unwrap_or_else(|| "none".to_string())
        };
        let mut border_color = if color_acc.mixed {
            "auto".to_string()
        } else {
            color_acc.value.unwrap_or_else(|| "auto".to_string())
        };
        if border_style == "none" {
            border_width = 0;
            border_color = "auto".to_string();
        }

        TextStyle {
            font_family,
            font_size,
            bold,
            italic,
            color,
            background_color,
            underline,
            strike,
            dstrike,
            caps,
            small_caps,
            shadow,
            spacing,
            vertical_align,
            padding,
            border_width,
            border_style,
            border_color,
        }
    }

    /// Port of `DOCXStyle.serialize` + `TextStyle.serialize` combined:
    /// the standalone `<w:style w:type="character">` element (not yet
    /// appended into a `w:styles` document -- that's `StylesManager`'s
    /// job, not ported yet). `is_normal_style`/`normal_style_id`
    /// stand in for Python's `self is normal_style`
    /// identity-comparison/`normal_style.id`.
    pub fn serialize(
        &self,
        id: &str,
        name: &str,
        is_normal_style: bool,
        normal_style: &TextStyle,
        normal_style_id: &str,
    ) -> Element {
        let mut style = Element::new("w:style")
            .attr("w:styleId", id)
            .attr("w:type", "character")
            .with(Element::new("w:name").attr("w:val", name));
        if !is_normal_style {
            style = style.with(Element::new("w:basedOn").attr("w:val", normal_style_id));
        }
        let mut rpr = Element::new("w:rPr");
        self.serialize_properties(&mut rpr, is_normal_style, normal_style);
        if !rpr.is_empty() {
            style = style.with(rpr);
        }
        style
    }

    /// Port of `TextStyle.serialize_properties`: appends every
    /// property that differs from `normal_style` (or, for the Normal
    /// style itself, every property unconditionally -- `is_normal_style`
    /// mirrors Python's `self is normal_style`).
    pub fn serialize_properties(
        &self,
        rpr: &mut Element,
        is_normal_style: bool,
        normal_style: &TextStyle,
    ) {
        if (is_normal_style || self.font_family != normal_style.font_family)
            && self.font_family.is_some()
        {
            // Python sets ascii/cs/eastAsia/hAnsi to `self.font_family`
            // unconditionally, which would pass `None` to lxml (a
            // TypeError there) if it were ever actually `None` on this
            // branch -- disclosed deviation: omit `w:rFonts` entirely
            // rather than reproduce a crash no real font stack should
            // trigger (`css_font_family_to_docx` only returns `None`
            // for a genuinely empty `font-family` declaration).
            let family = self.font_family.as_deref().unwrap();
            rpr.append(
                Element::new("w:rFonts")
                    .attr("w:ascii", family)
                    .attr("w:cs", family)
                    .attr("w:eastAsia", family)
                    .attr("w:hAnsi", family),
            );
        }

        if is_normal_style || normal_style.font_size != self.font_size {
            let val = self.font_size.unwrap_or(0).to_string();
            rpr.append(Element::new("w:sz").attr("w:val", val.clone()));
            rpr.append(Element::new("w:szCs").attr("w:val", val));
        }
        if is_normal_style || normal_style.bold != self.bold {
            rpr.append(Element::new("w:b").attr("w:val", bmap(self.bold)));
            rpr.append(Element::new("w:bCs").attr("w:val", bmap(self.bold)));
        }
        if is_normal_style || normal_style.italic != self.italic {
            rpr.append(Element::new("w:i").attr("w:val", bmap(self.italic)));
            rpr.append(Element::new("w:iCs").attr("w:val", bmap(self.italic)));
        }

        let changed = |same: bool| is_normal_style || !same;

        if changed(self.color == normal_style.color) {
            rpr.append(Element::new("w:color").attr(
                "w:val",
                self.color.clone().unwrap_or_else(|| "auto".to_string()),
            ));
        }
        if changed(self.background_color == normal_style.background_color) {
            rpr.append(
                Element::new("w:shd").attr(
                    "w:fill",
                    self.background_color
                        .clone()
                        .unwrap_or_else(|| "auto".to_string()),
                ),
            );
        }
        if changed(self.underline == normal_style.underline) {
            let (u_style, u_color) = match self.underline.split_once(' ') {
                Some((s, c)) => (s.to_string(), c.to_string()),
                None => (self.underline.clone(), String::new()),
            };
            let mut u = Element::new("w:u").attr("w:val", u_style);
            if u_color != "auto" {
                u = u.attr("w:color", u_color);
            }
            rpr.append(u);
        }
        if changed(self.dstrike == normal_style.dstrike) {
            rpr.append(Element::new("w:dstrike").attr("w:val", bmap(self.dstrike)));
        }
        if changed(self.strike == normal_style.strike) {
            rpr.append(Element::new("w:strike").attr("w:val", bmap(self.strike)));
        }
        if changed(self.caps == normal_style.caps) {
            rpr.append(Element::new("w:caps").attr("w:val", bmap(self.caps)));
        }
        if changed(self.small_caps == normal_style.small_caps) {
            rpr.append(Element::new("w:smallCaps").attr("w:val", bmap(self.small_caps)));
        }
        if changed(self.shadow == normal_style.shadow) {
            rpr.append(Element::new("w:shadow").attr("w:val", bmap(self.shadow)));
        }
        if changed(self.spacing == normal_style.spacing) {
            rpr.append(
                Element::new("w:spacing").attr("w:val", self.spacing.unwrap_or(0).to_string()),
            );
        }

        if is_normal_style {
            let val = if matches!(self.vertical_align.as_str(), "superscript" | "subscript") {
                self.vertical_align.as_str()
            } else {
                "baseline"
            };
            rpr.append(Element::new("w:vertAlign").attr("w:val", val));
        } else if self.vertical_align != normal_style.vertical_align {
            if matches!(
                self.vertical_align.as_str(),
                "superscript" | "subscript" | "baseline"
            ) {
                rpr.append(Element::new("w:vertAlign").attr("w:val", self.vertical_align.clone()));
            } else {
                rpr.append(Element::new("w:position").attr("w:val", self.vertical_align.clone()));
            }
        }

        let mut bdr = Element::new("w:bdr");
        self.serialize_borders(&mut bdr, is_normal_style, normal_style);
        if !bdr.attrs.is_empty() {
            rpr.append(bdr);
        }
    }

    /// Port of `TextStyle.serialize_borders`.
    fn serialize_borders(
        &self,
        bdr: &mut Element,
        is_normal_style: bool,
        normal_style: &TextStyle,
    ) {
        if is_normal_style || self.padding != normal_style.padding {
            bdr.set("w:space", self.padding.to_string());
        }
        if is_normal_style || self.border_width != normal_style.border_width {
            bdr.set("w:sz", self.border_width.to_string());
        }
        if is_normal_style || self.border_style != normal_style.border_style {
            bdr.set("w:val", self.border_style.clone());
        }
        if is_normal_style || self.border_color != normal_style.border_color {
            bdr.set("w:color", self.border_color.clone());
        }
    }
}

/// `padding-{edge}` is one of the base `Style` class's dedicated
/// accessors (`paddingTop`/`paddingLeft`/...), already resolved to
/// points -- port of `css['padding-' + edge]`, kept as a free function
/// since [`crate::oeb::polish::style::Style`] doesn't expose it as a
/// single `by name` accessor.
fn padding_for(css: &Style, edge: &str) -> f64 {
    match edge {
        "left" => css.padding_left(),
        "top" => css.padding_top(),
        "right" => css.padding_right(),
        "bottom" => css.padding_bottom(),
        _ => 0.0,
    }
}

/// Port of `is_dropcaps`: a floated block short enough (fewer than two
/// child elements, under five characters of text) to be Word's
/// drop-cap frame instead of a real float.
pub fn is_dropcaps(dom: &Dom, html_tag: NodeId, tag_style: &Style) -> bool {
    let child_count = dom
        .children(html_tag)
        .iter()
        .filter(|&&c| dom.tag(c).is_some())
        .count();
    let text_len = dom.text_content(html_tag).chars().count();
    child_count < 2 && text_len < 5 && tag_style.get("float") == "left"
}

/// One box edge's padding/margin/border, in the DOCX-ready units each
/// field is actually serialized in -- padding in points, margin in
/// twips (twentieths of a point), border width in eighths of a point,
/// `LINE_STYLES`' `w:val` keyword, and a `RRGGBB`/`auto` colour.
/// Shared shape [`BlockStyle`]/`FloatSpec`/(not yet ported)
/// `tables.py`'s cell/row/table styles all read from
/// [`read_css_block_borders`], mirroring how Python's version writes
/// onto whatever `self` its caller passes via `setattr`.
///
/// `border_*_color` is `None` only in the `css: None` branch of
/// [`read_css_block_borders`] (Python literally stores `None` there,
/// distinct from the real `'auto'` string every other branch produces
/// via `convert_color(...) or 'auto'`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct BlockBorders {
    pub padding_left: i64,
    pub padding_top: i64,
    pub padding_right: i64,
    pub padding_bottom: i64,
    pub margin_left: i64,
    pub margin_top: i64,
    pub margin_right: i64,
    pub margin_bottom: i64,
    /// The raw, undeclared-inheritance `margin-{edge}` CSS text (`""`
    /// when unset) -- port of `css._style.get('margin-' + edge, '')`,
    /// needed later (not yet ported) to detect an `em`/`ex` margin and
    /// serialize `w:beforeLines`/`w:leftChars` instead of an absolute
    /// twips value.
    pub css_margin_left: String,
    pub css_margin_top: String,
    pub css_margin_right: String,
    pub css_margin_bottom: String,
    pub border_left_width: i64,
    pub border_top_width: i64,
    pub border_right_width: i64,
    pub border_bottom_width: i64,
    pub border_left_style: String,
    pub border_top_style: String,
    pub border_right_style: String,
    pub border_bottom_style: String,
    pub border_left_color: Option<String>,
    pub border_top_color: Option<String>,
    pub border_right_color: Option<String>,
    pub border_bottom_color: Option<String>,
}

impl BlockBorders {
    /// Port of Python's `getattr(self, 'margin_' + edge)` -- a
    /// by-edge-name accessor `BlockBorders`' flat fields don't offer
    /// natively (Rust has no runtime attribute reflection).
    fn margin(&self, edge: &str) -> i64 {
        match edge {
            "left" => self.margin_left,
            "top" => self.margin_top,
            "right" => self.margin_right,
            "bottom" => self.margin_bottom,
            _ => 0,
        }
    }

    fn css_margin(&self, edge: &str) -> &str {
        match edge {
            "left" => &self.css_margin_left,
            "top" => &self.css_margin_top,
            "right" => &self.css_margin_right,
            "bottom" => &self.css_margin_bottom,
            _ => "",
        }
    }

    fn padding(&self, edge: &str) -> i64 {
        match edge {
            "left" => self.padding_left,
            "top" => self.padding_top,
            "right" => self.padding_right,
            "bottom" => self.padding_bottom,
            _ => 0,
        }
    }

    fn border_width(&self, edge: &str) -> i64 {
        match edge {
            "left" => self.border_left_width,
            "top" => self.border_top_width,
            "right" => self.border_right_width,
            "bottom" => self.border_bottom_width,
            _ => 0,
        }
    }

    fn border_style(&self, edge: &str) -> &str {
        match edge {
            "left" => &self.border_left_style,
            "top" => &self.border_top_style,
            "right" => &self.border_right_style,
            "bottom" => &self.border_bottom_style,
            _ => "none",
        }
    }

    fn border_color(&self, edge: &str) -> Option<&str> {
        match edge {
            "left" => self.border_left_color.as_deref(),
            "top" => self.border_top_color.as_deref(),
            "right" => self.border_right_color.as_deref(),
            "bottom" => self.border_bottom_color.as_deref(),
            _ => None,
        }
    }
}

/// The lowercased raw `border-{edge}-style` CSS keyword, per edge --
/// `read_css_block_borders`'s `store_css_style=True` output, needed
/// only by `tables.py` (not yet ported) to tell "explicitly `none`"
/// apart from "never declared" when deciding whether a table's own
/// border settings should show through a cell.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct BlockBorderCssStyles {
    pub border_left_css_style: String,
    pub border_top_css_style: String,
    pub border_right_css_style: String,
    pub border_bottom_css_style: String,
}

fn margin_for(css: &Style, edge: &str) -> f64 {
    match edge {
        "left" => css.margin_left(),
        "top" => css.margin_top(),
        "right" => css.margin_right(),
        "bottom" => css.margin_bottom(),
        _ => 0.0,
    }
}

/// Reads one edge's padding/margin/border into `borders`/`css_styles`
/// -- port of `read_css_block_borders`. `css: None` matches Python's
/// own `css is None` branch (a block with no resolved style at all);
/// `store_css_style` matches Python's own parameter (only `tables.py`,
/// not yet ported, passes `true`).
pub fn read_css_block_borders(
    css: Option<&Style>,
    store_css_style: bool,
) -> (BlockBorders, Option<BlockBorderCssStyles>) {
    let Some(css) = css else {
        let borders = BlockBorders {
            border_left_width: 2,
            border_top_width: 2,
            border_right_width: 2,
            border_bottom_width: 2,
            border_left_style: "none".to_string(),
            border_top_style: "none".to_string(),
            border_right_style: "none".to_string(),
            border_bottom_style: "none".to_string(),
            ..Default::default()
        };
        let css_styles = store_css_style.then(|| BlockBorderCssStyles {
            border_left_css_style: "none".to_string(),
            border_top_css_style: "none".to_string(),
            border_right_css_style: "none".to_string(),
            border_bottom_css_style: "none".to_string(),
        });
        return (borders, css_styles);
    };

    let mut borders = BlockBorders::default();
    let mut css_styles = store_css_style.then(BlockBorderCssStyles::default);

    for edge in BORDER_EDGES {
        let padding = padding_for(css, edge).max(0.0) as i64;
        let margin = (margin_for(css, edge) * 20.0).max(0.0) as i64;
        let css_margin = css.own(&format!("margin-{edge}")).unwrap_or_default();
        let raw_w = border_width_value(css, edge);
        let width = ((raw_w * 8.0) as i64).clamp(2, 96);
        let color = convert_color(Some(&css.get(&format!("border-{edge}-color"))))
            .unwrap_or_else(|| "auto".to_string());
        let style_kw = css
            .get(&format!("border-{edge}-style"))
            .to_ascii_lowercase();
        let style = line_style(&style_kw).to_string();

        match edge {
            "left" => {
                borders.padding_left = padding;
                borders.margin_left = margin;
                borders.css_margin_left = css_margin;
                borders.border_left_width = width;
                borders.border_left_color = Some(color);
                borders.border_left_style = style;
                if let Some(s) = &mut css_styles {
                    s.border_left_css_style = style_kw;
                }
            }
            "top" => {
                borders.padding_top = padding;
                borders.margin_top = margin;
                borders.css_margin_top = css_margin;
                borders.border_top_width = width;
                borders.border_top_color = Some(color);
                borders.border_top_style = style;
                if let Some(s) = &mut css_styles {
                    s.border_top_css_style = style_kw;
                }
            }
            "right" => {
                borders.padding_right = padding;
                borders.margin_right = margin;
                borders.css_margin_right = css_margin;
                borders.border_right_width = width;
                borders.border_right_color = Some(color);
                borders.border_right_style = style;
                if let Some(s) = &mut css_styles {
                    s.border_right_css_style = style_kw;
                }
            }
            "bottom" => {
                borders.padding_bottom = padding;
                borders.margin_bottom = margin;
                borders.css_margin_bottom = css_margin;
                borders.border_bottom_width = width;
                borders.border_bottom_color = Some(color);
                borders.border_bottom_style = style;
                if let Some(s) = &mut css_styles {
                    s.border_bottom_css_style = style_kw;
                }
            }
            _ => unreachable!(),
        }
    }

    (borders, css_styles)
}

/// The paragraph-level CSS -> `w:pPr` property set -- port of
/// `BlockStyle`'s `ALL_PROPS` fields (everything `__init__` computes
/// from `css`, plus [`BlockBorders`] via [`read_css_block_borders`]).
/// Not yet ported: `BlockStyle.serialize`/`serialize_properties`
/// (real `w:pPr` XML) and `DOCXStyle`'s hash/dedup bookkeeping fields
/// (`id`/`name`/`next_style`) -- same scope split as [`TextStyle`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockStyle {
    pub borders: BlockBorders,
    /// Twentieths of a point.
    pub text_indent: i64,
    /// The raw `text-indent` CSS text, needed later (not yet ported)
    /// for the same `em`/`ex` special-casing `css_margin_*` is for.
    pub css_text_indent: Option<String>,
    /// Twentieths of a point.
    pub line_height: i64,
    pub background_color: Option<String>,
    pub text_align: String,
}

impl BlockStyle {
    /// Port of `BlockStyle.__init__`. `is_table_cell` zeroes every
    /// border/padding/margin (DOCX cell borders/spacing come from the
    /// table's own row/cell styles, not #ported yet, never the
    /// paragraph inside a cell) and skips `background_color`
    /// (inherited from the cell instead, via `parent_bg`).
    ///
    /// A few of Python's `try/except (TypeError, ValueError)` guards
    /// around `css.lineHeight`/`css['white-space']`/`css['text-align']`
    /// are not reproduced: they exist because Python's `Style`
    /// properties can raise for values `_unit_convert`/`_get` can't
    /// coerce, but every equivalent [`Style`] accessor here already
    /// returns a plain `f64`/`String` with its own internal fallback
    /// (`unwrap_or(0.0)`/an empty string), so the exception path is
    /// unreachable in this port -- there's no distinguishable failure
    /// state left to catch.
    pub fn from_css(css: Option<&Style>, is_table_cell: bool, parent_bg: Option<&str>) -> Self {
        let (mut borders, _) = read_css_block_borders(css, false);
        if is_table_cell {
            for edge in BORDER_EDGES {
                match edge {
                    "left" => {
                        borders.border_left_style = "none".to_string();
                        borders.border_left_width = 0;
                        borders.padding_left = 0;
                        borders.margin_left = 0;
                    }
                    "top" => {
                        borders.border_top_style = "none".to_string();
                        borders.border_top_width = 0;
                        borders.padding_top = 0;
                        borders.margin_top = 0;
                    }
                    "right" => {
                        borders.border_right_style = "none".to_string();
                        borders.border_right_width = 0;
                        borders.padding_right = 0;
                        borders.margin_right = 0;
                    }
                    "bottom" => {
                        borders.border_bottom_style = "none".to_string();
                        borders.border_bottom_width = 0;
                        borders.padding_bottom = 0;
                        borders.margin_bottom = 0;
                    }
                    _ => unreachable!(),
                }
            }
        }

        let Some(css) = css else {
            return BlockStyle {
                borders,
                text_indent: 0,
                css_text_indent: None,
                line_height: 280,
                background_color: None,
                text_align: "left".to_string(),
            };
        };

        let (text_indent, css_text_indent) = match css.item("text-indent") {
            ItemValue::Number(n) => ((n * 20.0) as i64, Some(css.get("text-indent"))),
            ItemValue::Text(_) => (0, None),
        };

        let line_height = ((css.line_height() * 20.0).max(0.0)) as i64;

        let background_color = if is_table_cell {
            None
        } else {
            convert_color(css.background_color().as_deref())
                .or_else(|| parent_bg.map(str::to_string))
        };

        let white_space = css.get("white-space").to_ascii_lowercase();
        let preserve_whitespace = matches!(white_space.as_str(), "pre" | "pre-wrap");

        let mut text_align = css.get("text-align").to_ascii_lowercase();
        if preserve_whitespace {
            text_align = "start".to_string();
        }
        let text_align = match text_align.as_str() {
            "start" | "left" => "left",
            "end" | "right" => "right",
            "center" | "centre" => "center",
            "justify" => "both",
            _ => "left",
        }
        .to_string();

        BlockStyle {
            borders,
            text_indent,
            css_text_indent,
            line_height,
            background_color,
            text_align,
        }
    }

    /// Port of `DOCXStyle.serialize` + `BlockStyle.serialize`
    /// combined -- see [`TextStyle::serialize`] for why `id`/`name`/
    /// `is_normal_style`/`next_style` are parameters, not fields.
    #[allow(clippy::too_many_arguments)]
    pub fn serialize(
        &self,
        id: &str,
        name: &str,
        is_normal_style: bool,
        normal_style: &BlockStyle,
        normal_style_id: &str,
        next_style: Option<&str>,
    ) -> Element {
        let mut style = Element::new("w:style")
            .attr("w:styleId", id)
            .attr("w:type", "paragraph")
            .with(Element::new("w:name").attr("w:val", name));
        if !is_normal_style {
            style = style.with(Element::new("w:basedOn").attr("w:val", normal_style_id));
        }
        let mut ppr = Element::new("w:pPr");
        self.serialize_properties(&mut ppr, is_normal_style, normal_style, next_style);
        if !ppr.is_empty() {
            style = style.with(ppr);
        }
        style
    }

    /// Port of `BlockStyle.serialize_properties`.
    pub fn serialize_properties(
        &self,
        ppr: &mut Element,
        is_normal_style: bool,
        normal_style: &BlockStyle,
        next_style: Option<&str>,
    ) {
        let mut spacing = Element::new("w:spacing");
        for (edge, attr) in [("top", "before"), ("bottom", "after")] {
            let css_margin = self.borders.css_margin(edge);
            match parse_css_length(Some(css_margin)) {
                Some((val, unit)) if unit == "em" || unit == "ex" => {
                    let lines = ((val * if unit == "ex" { 50.0 } else { 100.0 }) as i64).max(0);
                    if (is_normal_style && lines > 0)
                        || css_margin != normal_style.borders.css_margin(edge)
                    {
                        spacing.set(format!("w:{attr}Lines"), lines.to_string());
                    }
                }
                _ => {
                    let val = self.borders.margin(edge);
                    if (is_normal_style && val > 0) || val != normal_style.borders.margin(edge) {
                        spacing.set(format!("w:{attr}"), val.to_string());
                    }
                }
            }
        }

        if is_normal_style || self.line_height != normal_style.line_height {
            spacing.set("w:line", self.line_height.to_string());
            spacing.set("w:lineRule", "atLeast");
        }

        if !spacing.attrs.is_empty() {
            ppr.append(spacing);
        }

        let mut ind = Element::new("w:ind");
        for edge in ["left", "right"] {
            let css_margin = self.borders.css_margin(edge);
            match parse_css_length(Some(css_margin)) {
                Some((val, unit)) if unit == "em" || unit == "ex" => {
                    let chars = ((val * if unit == "ex" { 50.0 } else { 100.0 }) as i64).max(0);
                    if (is_normal_style && chars > 0)
                        || css_margin != normal_style.borders.css_margin(edge)
                    {
                        ind.set(format!("w:{edge}Chars"), chars.to_string());
                    }
                }
                _ => {
                    let val = self.borders.margin(edge);
                    if (is_normal_style && val > 0) || val != normal_style.borders.margin(edge) {
                        ind.set(format!("w:{edge}"), val.to_string());
                        ind.set(format!("w:{edge}Chars"), "0");
                    }
                }
            }
        }
        match parse_css_length(self.css_text_indent.as_deref()) {
            Some((css_val, unit)) if unit == "em" || unit == "ex" => {
                let chars = (css_val * if unit == "ex" { 50.0 } else { 100.0 }) as i64;
                if css_val >= 0.0 {
                    if (is_normal_style && chars > 0)
                        || self.css_text_indent != normal_style.css_text_indent
                    {
                        ind.set("w:firstLineChars", chars.to_string());
                    }
                } else if (is_normal_style && chars < 0)
                    || self.css_text_indent != normal_style.css_text_indent
                {
                    ind.set("w:hangingChars", chars.unsigned_abs().to_string());
                }
            }
            _ => {
                let val = self.text_indent;
                if val >= 0 {
                    if (is_normal_style && val > 0) || val != normal_style.text_indent {
                        ind.set("w:firstLine", val.to_string());
                        ind.set("w:firstLineChars", "0");
                    }
                } else if (is_normal_style && val < 0) || val != normal_style.text_indent {
                    ind.set("w:hanging", val.unsigned_abs().to_string());
                    ind.set("w:hangingChars", "0");
                }
            }
        }
        if !ind.attrs.is_empty() {
            ppr.append(ind);
        }

        if (is_normal_style && self.background_color.is_some())
            || self.background_color != normal_style.background_color
        {
            ppr.append(
                Element::new("w:shd")
                    .attr("w:val", "clear")
                    .attr("w:color", "auto")
                    .attr(
                        "w:fill",
                        self.background_color
                            .clone()
                            .unwrap_or_else(|| "auto".to_string()),
                    ),
            );
        }

        let pbdr = self.serialize_borders(is_normal_style, normal_style);
        if pbdr.child_count() > 0 {
            ppr.append(pbdr);
        }

        if is_normal_style || self.text_align != normal_style.text_align {
            ppr.append(Element::new("w:jc").attr("w:val", self.text_align.clone()));
        }

        if !is_normal_style {
            if let Some(next) = next_style {
                ppr.append(Element::new("w:next").attr("w:val", next));
            }
        }
    }

    /// Port of `BlockStyle.serialize_borders`: one `<w:{edge}>` child
    /// per edge, inside a `<w:pBdr>` wrapper, each only carrying the
    /// attributes that differ from `normal_style` (or, for the Normal
    /// style itself, whichever attributes are actually non-default).
    fn serialize_borders(&self, is_normal_style: bool, normal_style: &BlockStyle) -> Element {
        let mut pbdr = Element::new("w:pBdr");
        for edge in BORDER_EDGES {
            let mut e = Element::new(format!("w:{edge}"));
            let padding = self.borders.padding(edge);
            if (is_normal_style && padding > 0) || padding != normal_style.borders.padding(edge) {
                e.set("w:space", padding.to_string());
            }
            let width = self.borders.border_width(edge);
            let bstyle = self.borders.border_style(edge);
            if (is_normal_style && width > 0 && bstyle != "none")
                || width != normal_style.borders.border_width(edge)
                || bstyle != normal_style.borders.border_style(edge)
            {
                e.set("w:val", bstyle);
                e.set("w:sz", width.to_string());
                if let Some(color) = self.borders.border_color(edge) {
                    e.set("w:color", color);
                }
            }
            if !e.attrs.is_empty() {
                pbdr.append(e);
            }
        }
        pbdr
    }
}

/// A floated (`float: left`/`right`) block's frame geometry -- port
/// of `FloatSpec`. `blocks` (the paragraphs the frame wraps, appended
/// externally as `Block`s are built) and `serialize` (real
/// `w:framePr` XML) aren't ported yet -- both need `from_html.py`'s
/// not-yet-ported `Block` type/`docx/writer/xml.rs`'s element builder.
#[derive(Debug, Clone, PartialEq)]
pub struct FloatSpec {
    pub is_dropcaps: bool,
    pub dropcaps_lines: Option<i64>,
    pub x_align: Option<String>,
    /// Twentieths of a point.
    pub w: Option<i64>,
    /// Twentieths of a point.
    pub h: Option<i64>,
    pub h_rule: Option<String>,
    /// Twentieths of a point.
    pub h_space: Option<i64>,
    /// Twentieths of a point.
    pub v_space: Option<i64>,
    pub borders: BlockBorders,
}

impl FloatSpec {
    /// Port of `FloatSpec.__init__`.
    pub fn from_css(dom: &Dom, html_tag: NodeId, tag_style: &Style) -> Self {
        let is_dropcaps = is_dropcaps(dom, html_tag, tag_style);

        let (dropcaps_lines, x_align, w, h, h_rule, h_space, v_space) = if is_dropcaps {
            (Some(3), None, None, None, None, None, None)
        } else {
            let x_align = Some(tag_style.get("float"));

            let w = if tag_style.get("width") != "auto" {
                let min_width = match tag_style.item("min-width") {
                    ItemValue::Number(n) => n,
                    ItemValue::Text(_) => 0.0,
                };
                Some((20.0 * min_width.max(tag_style.width())) as i64)
            } else {
                None
            };

            let (h_rule, h) = if tag_style.get("height") == "auto" {
                ("auto".to_string(), None)
            } else {
                let min_height = match tag_style.item("min-height") {
                    ItemValue::Number(n) => n,
                    ItemValue::Text(_) => 0.0,
                };
                let (rule, raw_h) = if min_height > 0.0 {
                    ("atLeast", min_height)
                } else {
                    ("exact", tag_style.height())
                };
                (rule.to_string(), Some((20.0 * raw_h) as i64))
            };

            let h_space =
                Some((20.0 * tag_style.margin_right().max(tag_style.margin_left())) as i64);
            let v_space =
                Some((20.0 * tag_style.margin_top().max(tag_style.margin_bottom())) as i64);

            (None, x_align, w, h, Some(h_rule), h_space, v_space)
        };

        let (borders, _) = read_css_block_borders(Some(tag_style), false);

        FloatSpec {
            is_dropcaps,
            dropcaps_lines,
            x_align,
            w,
            h,
            h_rule,
            h_space,
            v_space,
            borders,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::polish::cascade::{PropertyValue, ResolvedStyles};
    use crate::oeb::polish::style::Profile;
    use std::collections::HashMap;

    fn make(html: &str) -> Dom {
        Dom::parse(html)
    }

    fn resolved_with(entries: &[(NodeId, &[(&str, &str)])]) -> ResolvedStyles {
        let mut style_map = HashMap::new();
        for &(id, props) in entries {
            let mut m = HashMap::new();
            for &(k, v) in props {
                m.insert(k.to_string(), PropertyValue::new(v, None, false));
            }
            style_map.insert(id, m);
        }
        ResolvedStyles {
            style_map,
            pseudo_style_map: HashMap::new(),
        }
    }

    fn find(dom: &Dom, tag: &str) -> NodeId {
        dom.preorder_elements(dom.root)
            .into_iter()
            .find(|&id| dom.tag(id) == Some(tag))
            .unwrap()
    }

    #[test]
    fn css_font_family_to_docx_maps_generics_and_keeps_real_names() {
        assert_eq!(
            css_font_family_to_docx("serif"),
            Some("Cambria".to_string())
        );
        assert_eq!(
            css_font_family_to_docx("\"Times New Roman\", serif"),
            Some("Times New Roman".to_string())
        );
        assert_eq!(css_font_family_to_docx(""), None);
        assert_eq!(
            css_font_family_to_docx("inherit"),
            None,
            "inherit stops the scan before any name is yielded"
        );
    }

    #[test]
    fn convert_underline_combines_style_and_color() {
        let items: HashSet<&str> = ["underline", "wavy", "red"].into_iter().collect();
        // `convert_underline`'s "color" branch runs every unrecognized
        // token through `convert_color`, which normalizes "red" to
        // its hex form -- not the literal keyword.
        assert_eq!(convert_underline(&items), "wave FF0000");
    }

    #[test]
    fn convert_underline_is_empty_with_no_underline_token() {
        let items: HashSet<&str> = ["line-through"].into_iter().collect();
        assert_eq!(convert_underline(&items), "");
    }

    #[test]
    fn line_style_maps_every_keyword() {
        assert_eq!(line_style("solid"), "single");
        assert_eq!(line_style("groove"), "threeDEngrave");
        assert_eq!(line_style("bogus"), "none");
    }

    #[test]
    fn text_style_reads_bold_italic_and_color() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(
            p,
            &[
                ("font-weight", "bold"),
                ("font-style", "italic"),
                ("color", "#ff0000"),
            ],
        )]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        let ts = TextStyle::from_css(&style, false);
        assert!(ts.bold);
        assert!(ts.italic);
        assert_eq!(ts.color.as_deref(), Some("FF0000"));
    }

    #[test]
    fn text_style_numeric_font_weight_of_700_or_more_is_bold() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(p, &[("font-weight", "700")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        assert!(TextStyle::from_css(&style, false).bold);
    }

    #[test]
    fn text_style_font_size_is_half_points() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(p, &[("font-size", "10pt")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        assert_eq!(TextStyle::from_css(&style, false).font_size, Some(20));
    }

    #[test]
    fn text_style_parent_style_never_carries_background_or_borders() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(
            p,
            &[
                ("background-color", "#00ff00"),
                ("border-left-style", "solid"),
            ],
        )]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        let ts = TextStyle::from_css(&style, true);
        assert_eq!(ts.background_color, None);
        assert_eq!(ts.border_style, "none");
        assert_eq!(ts.border_width, 0);
    }

    #[test]
    fn text_style_uniform_border_edges_are_kept_mixed_edges_reset() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(
            p,
            &[
                ("border-left-style", "solid"),
                ("border-top-style", "solid"),
                ("border-right-style", "solid"),
                ("border-bottom-style", "solid"),
            ],
        )]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        let ts = TextStyle::from_css(&style, false);
        assert_eq!(ts.border_style, "single");
    }

    #[test]
    fn text_style_mixed_border_styles_across_edges_reset_to_none() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(
            p,
            &[
                ("border-left-style", "solid"),
                ("border-top-style", "dashed"),
                ("border-right-style", "solid"),
                ("border-bottom-style", "solid"),
            ],
        )]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        let ts = TextStyle::from_css(&style, false);
        assert_eq!(ts.border_style, "none");
        assert_eq!(ts.border_width, 0);
        assert_eq!(ts.border_color, "auto");
    }

    #[test]
    fn text_style_vertical_align_keyword_maps_to_superscript() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(p, &[("vertical-align", "super")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        assert_eq!(
            TextStyle::from_css(&style, false).vertical_align,
            "superscript"
        );
    }

    #[test]
    fn text_style_equal_inputs_hash_and_compare_equal() {
        let dom = make("<html><body><p>x</p><p>y</p></body></html>");
        let ps: Vec<NodeId> = dom
            .preorder_elements(dom.root)
            .into_iter()
            .filter(|&id| dom.tag(id) == Some("p"))
            .collect();
        let resolved = resolved_with(&[(ps[0], &[("color", "red")]), (ps[1], &[("color", "red")])]);
        let profile = Profile::default();
        let a = TextStyle::from_css(&Style::new(&dom, &resolved, &profile, ps[0]), false);
        let b = TextStyle::from_css(&Style::new(&dom, &resolved, &profile, ps[1]), false);
        assert_eq!(a, b);
        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&b));
    }

    fn plain_text_style() -> TextStyle {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        TextStyle::from_css(&Style::new(&dom, &resolved, &profile, p), false)
    }

    #[test]
    fn text_style_serialize_properties_for_the_normal_style_emits_everything() {
        let normal = plain_text_style();
        let mut rpr = Element::new("w:rPr");
        normal.serialize_properties(&mut rpr, true, &normal);
        assert!(rpr.children_named("w:sz").next().is_some());
        assert!(rpr.children_named("w:b").next().is_some());
        assert!(rpr.children_named("w:vertAlign").next().is_some());
    }

    #[test]
    fn text_style_serialize_properties_for_a_non_normal_style_only_emits_differences() {
        let normal = plain_text_style();
        let mut bold = normal.clone();
        bold.bold = true;
        let mut rpr = Element::new("w:rPr");
        bold.serialize_properties(&mut rpr, false, &normal);
        assert!(rpr.children_named("w:b").next().is_some());
        assert!(
            rpr.children_named("w:i").next().is_none(),
            "italic didn't change, so it's not re-emitted"
        );
    }

    #[test]
    fn text_style_underline_with_a_real_color_emits_both_attributes() {
        let mut style = plain_text_style();
        style.underline = "single FF0000".to_string();
        let normal = plain_text_style();
        let mut rpr = Element::new("w:rPr");
        style.serialize_properties(&mut rpr, false, &normal);
        let u = rpr.children_named("w:u").next().unwrap();
        assert_eq!(u.get("w:val"), Some("single"));
        assert_eq!(u.get("w:color"), Some("FF0000"));
    }

    #[test]
    fn text_style_underline_auto_color_omits_the_color_attribute() {
        let mut style = plain_text_style();
        style.underline = "single auto".to_string();
        let normal = plain_text_style();
        let mut rpr = Element::new("w:rPr");
        style.serialize_properties(&mut rpr, false, &normal);
        let u = rpr.children_named("w:u").next().unwrap();
        assert_eq!(u.get("w:color"), None);
    }

    #[test]
    fn text_style_vertical_align_keyword_uses_vert_align_element() {
        let mut style = plain_text_style();
        style.vertical_align = "superscript".to_string();
        let normal = plain_text_style();
        let mut rpr = Element::new("w:rPr");
        style.serialize_properties(&mut rpr, false, &normal);
        let va = rpr.children_named("w:vertAlign").next().unwrap();
        assert_eq!(va.get("w:val"), Some("superscript"));
    }

    #[test]
    fn text_style_vertical_align_raw_offset_uses_position_element() {
        let mut style = plain_text_style();
        style.vertical_align = "6".to_string();
        let normal = plain_text_style();
        let mut rpr = Element::new("w:rPr");
        style.serialize_properties(&mut rpr, false, &normal);
        assert!(rpr.children_named("w:vertAlign").next().is_none());
        let pos = rpr.children_named("w:position").next().unwrap();
        assert_eq!(pos.get("w:val"), Some("6"));
    }

    #[test]
    fn text_style_serialize_wraps_style_id_name_and_based_on() {
        let normal = plain_text_style();
        let mut other = normal.clone();
        other.bold = true;
        let el = other.serialize("Text1", "1 Text", false, &normal, "Normal");
        assert_eq!(el.name, "w:style");
        assert_eq!(el.get("w:styleId"), Some("Text1"));
        assert_eq!(el.get("w:type"), Some("character"));
        let name_el = el.children_named("w:name").next().unwrap();
        assert_eq!(name_el.get("w:val"), Some("1 Text"));
        let based_on = el.children_named("w:basedOn").next().unwrap();
        assert_eq!(based_on.get("w:val"), Some("Normal"));
    }

    #[test]
    fn text_style_serialize_normal_style_has_no_based_on() {
        let normal = plain_text_style();
        let el = normal.serialize("Normal", "Normal", true, &normal, "Normal");
        assert!(el.children_named("w:basedOn").next().is_none());
    }

    #[test]
    fn is_dropcaps_matches_a_short_floated_element() {
        let dom = make(r#"<html><body><span style="float:left">A</span></body></html>"#);
        let span = find(&dom, "span");
        let resolved = resolved_with(&[(span, &[("float", "left")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, span);
        assert!(is_dropcaps(&dom, span, &style));
    }

    #[test]
    fn is_dropcaps_is_false_for_longer_text() {
        let dom = make(r#"<html><body><span style="float:left">Hello</span></body></html>"#);
        let span = find(&dom, "span");
        let resolved = resolved_with(&[(span, &[("float", "left")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, span);
        assert!(!is_dropcaps(&dom, span, &style));
    }

    #[test]
    fn read_css_block_borders_with_no_css_uses_the_two_point_none_defaults() {
        let (borders, css_styles) = read_css_block_borders(None, false);
        assert_eq!(borders.border_left_width, 2);
        assert_eq!(borders.border_left_style, "none");
        assert_eq!(borders.border_left_color, None);
        assert_eq!(borders.padding_left, 0);
        assert!(css_styles.is_none());
    }

    #[test]
    fn read_css_block_borders_reads_every_edge_from_real_css() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(
            p,
            &[
                ("border-left-style", "solid"),
                ("border-left-color", "#00ff00"),
                ("padding-left", "5pt"),
                ("margin-left", "10pt"),
            ],
        )]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        let (borders, _) = read_css_block_borders(Some(&style), false);
        assert_eq!(borders.border_left_style, "single");
        assert_eq!(borders.border_left_color.as_deref(), Some("00FF00"));
        assert_eq!(borders.padding_left, 5);
        assert_eq!(borders.margin_left, 200, "10pt in twentieths of a point");
    }

    #[test]
    fn read_css_block_borders_store_css_style_captures_the_lowercased_keyword() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(p, &[("border-top-style", "DASHED")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        let (_, css_styles) = read_css_block_borders(Some(&style), true);
        assert_eq!(css_styles.unwrap().border_top_css_style, "dashed");
    }

    #[test]
    fn block_style_with_no_css_uses_the_hardcoded_defaults() {
        let bs = BlockStyle::from_css(None, false, None);
        assert_eq!(bs.text_indent, 0);
        assert_eq!(bs.line_height, 280);
        assert_eq!(bs.background_color, None);
        assert_eq!(bs.text_align, "left");
    }

    #[test]
    fn block_style_table_cell_zeroes_borders_and_background() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(
            p,
            &[
                ("border-left-style", "solid"),
                ("background-color", "#ff0000"),
            ],
        )]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        let bs = BlockStyle::from_css(Some(&style), true, None);
        assert_eq!(bs.borders.border_left_style, "none");
        assert_eq!(bs.background_color, None);
    }

    #[test]
    fn block_style_background_falls_back_to_the_parent_when_unset() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        let bs = BlockStyle::from_css(Some(&style), false, Some("112233"));
        assert_eq!(bs.background_color.as_deref(), Some("112233"));
    }

    #[test]
    fn block_style_text_align_maps_start_and_end_to_left_and_right() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(p, &[("text-align", "end")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        assert_eq!(
            BlockStyle::from_css(Some(&style), false, None).text_align,
            "right"
        );
    }

    #[test]
    fn block_style_preserved_whitespace_forces_start_alignment() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(p, &[("white-space", "pre"), ("text-align", "center")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        // preserve_whitespace forces `aval = 'start'`, which maps to left.
        assert_eq!(
            BlockStyle::from_css(Some(&style), false, None).text_align,
            "left"
        );
    }

    fn block_style_of(props: &[(&str, &str)]) -> BlockStyle {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(p, props)]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        BlockStyle::from_css(Some(&style), false, None)
    }

    #[test]
    fn block_style_serialize_properties_for_the_normal_style_emits_line_spacing_and_alignment() {
        let normal = block_style_of(&[]);
        let mut ppr = Element::new("w:pPr");
        normal.serialize_properties(&mut ppr, true, &normal, None);
        let spacing = ppr.children_named("w:spacing").next().unwrap();
        assert_eq!(
            spacing.get("w:line"),
            Some(normal.line_height.to_string()).as_deref()
        );
        let jc = ppr.children_named("w:jc").next().unwrap();
        assert_eq!(jc.get("w:val"), Some("left"));
    }

    #[test]
    fn block_style_serialize_properties_only_emits_alignment_when_it_differs() {
        let normal = block_style_of(&[]);
        let centered = block_style_of(&[("text-align", "center")]);
        let mut ppr = Element::new("w:pPr");
        centered.serialize_properties(&mut ppr, false, &normal, None);
        let jc = ppr.children_named("w:jc").next().unwrap();
        assert_eq!(jc.get("w:val"), Some("center"));

        let mut ppr2 = Element::new("w:pPr");
        normal.serialize_properties(&mut ppr2, false, &normal, None);
        assert!(
            ppr2.children_named("w:jc").next().is_none(),
            "identical alignment to normal_style is not re-emitted"
        );
    }

    #[test]
    fn block_style_serialize_properties_em_margin_uses_lines_not_absolute_twips() {
        let normal = block_style_of(&[]);
        let spaced = block_style_of(&[("margin-top", "2em")]);
        let mut ppr = Element::new("w:pPr");
        spaced.serialize_properties(&mut ppr, false, &normal, None);
        let spacing = ppr.children_named("w:spacing").next().unwrap();
        assert_eq!(spacing.get("w:beforeLines"), Some("200"), "2em * 100");
        assert_eq!(spacing.get("w:before"), None);
    }

    #[test]
    fn block_style_serialize_properties_positive_text_indent_sets_first_line() {
        let normal = block_style_of(&[]);
        let indented = block_style_of(&[("text-indent", "10pt")]);
        let mut ppr = Element::new("w:pPr");
        indented.serialize_properties(&mut ppr, false, &normal, None);
        let ind = ppr.children_named("w:ind").next().unwrap();
        assert_eq!(ind.get("w:firstLine"), Some("200"), "10pt * 20 twentieths");
        assert_eq!(ind.get("w:firstLineChars"), Some("0"));
    }

    #[test]
    fn block_style_serialize_properties_negative_text_indent_sets_hanging() {
        let normal = block_style_of(&[]);
        let hanging = block_style_of(&[("text-indent", "-10pt")]);
        let mut ppr = Element::new("w:pPr");
        hanging.serialize_properties(&mut ppr, false, &normal, None);
        let ind = ppr.children_named("w:ind").next().unwrap();
        assert_eq!(ind.get("w:hanging"), Some("200"));
        assert_eq!(ind.get("w:hangingChars"), Some("0"));
    }

    #[test]
    fn block_style_serialize_properties_next_style_only_appears_on_non_normal_styles() {
        let normal = block_style_of(&[]);
        let other = block_style_of(&[("text-align", "right")]);
        let mut ppr = Element::new("w:pPr");
        other.serialize_properties(&mut ppr, false, &normal, Some("Body Text"));
        let next = ppr.children_named("w:next").next().unwrap();
        assert_eq!(next.get("w:val"), Some("Body Text"));

        let mut ppr2 = Element::new("w:pPr");
        normal.serialize_properties(&mut ppr2, true, &normal, Some("Body Text"));
        assert!(ppr2.children_named("w:next").next().is_none());
    }

    #[test]
    fn block_style_serialize_borders_emits_one_child_per_declared_edge() {
        let normal = block_style_of(&[]);
        let bordered = block_style_of(&[
            ("border-left-style", "solid"),
            ("border-left-color", "#ff0000"),
        ]);
        let mut ppr = Element::new("w:pPr");
        bordered.serialize_properties(&mut ppr, false, &normal, None);
        let pbdr = ppr.children_named("w:pBdr").next().unwrap();
        let left = pbdr.children_named("w:left").next().unwrap();
        assert_eq!(left.get("w:val"), Some("single"));
        assert_eq!(left.get("w:color"), Some("FF0000"));
        assert!(pbdr.children_named("w:top").next().is_none());
    }

    #[test]
    fn block_style_serialize_wraps_style_id_name_and_based_on() {
        let normal = block_style_of(&[]);
        let other = block_style_of(&[("text-align", "right")]);
        let el = other.serialize("Para1", "1 Para", false, &normal, "Normal", None);
        assert_eq!(el.name, "w:style");
        assert_eq!(el.get("w:styleId"), Some("Para1"));
        assert_eq!(el.get("w:type"), Some("paragraph"));
        let based_on = el.children_named("w:basedOn").next().unwrap();
        assert_eq!(based_on.get("w:val"), Some("Normal"));
    }

    #[test]
    fn float_spec_reads_dropcaps() {
        let dom = make(r#"<html><body><span style="float:left">A</span></body></html>"#);
        let span = find(&dom, "span");
        let resolved = resolved_with(&[(span, &[("float", "left")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, span);
        let fs = FloatSpec::from_css(&dom, span, &style);
        assert!(fs.is_dropcaps);
        assert_eq!(fs.dropcaps_lines, Some(3));
    }

    #[test]
    fn float_spec_reads_a_real_float_geometry() {
        let dom =
            make(r#"<html><body><div style="float:left">Hello there, world!</div></body></html>"#);
        let div = find(&dom, "div");
        let resolved = resolved_with(&[(
            div,
            &[("float", "left"), ("width", "100pt"), ("height", "50pt")],
        )]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, div);
        let fs = FloatSpec::from_css(&dom, div, &style);
        assert!(!fs.is_dropcaps);
        assert_eq!(fs.x_align.as_deref(), Some("left"));
        assert_eq!(fs.w, Some(2000), "100pt * 20");
        assert_eq!(fs.h, Some(1000), "50pt * 20");
        assert_eq!(fs.h_rule.as_deref(), Some("exact"));
    }

    #[test]
    fn float_spec_auto_height_sets_h_rule_auto_with_no_height() {
        let dom =
            make(r#"<html><body><div style="float:left">Hello there, world!</div></body></html>"#);
        let div = find(&dom, "div");
        let resolved = resolved_with(&[(div, &[("float", "left"), ("height", "auto")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, div);
        let fs = FloatSpec::from_css(&dom, div, &style);
        assert_eq!(fs.h_rule.as_deref(), Some("auto"));
        assert_eq!(fs.h, None);
    }
}
