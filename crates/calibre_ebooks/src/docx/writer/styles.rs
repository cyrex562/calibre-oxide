//! CSS -> OOXML run-property conversion: the data-model half of
//! `docx/writer/styles.py`'s `TextStyle`.
//!
//! Port of the pure CSS-reading half of `TextStyle.__init__` plus its
//! module-level helpers (`css_font_family_to_docx`/
//! `parse_css_font_family`, `convert_underline`, `bmap`, `LINE_STYLES`).
//! `TextStyle.serialize`/`serialize_properties` (writing the resolved
//! properties out as real `w:rPr` XML), `DOCXStyle`'s hash/dedup
//! machinery, `BlockStyle`/`FloatSpec`/`DescendantTextStyle`, and
//! `StylesManager` (deduplication + `w:styles` assembly) are not
//! ported yet -- see issue #23's own tracking notes.
//!
//! Reads against [`crate::oeb::polish::style::Style`] -- the seam
//! issue #132 needed, not the stub `oeb::stylizer::Stylizer` its own
//! docs once assumed was still the state of things (see
//! `oeb/polish/style.rs`'s module docs for that correction).

use std::collections::HashSet;

use crate::dom::{Dom, NodeId};
use crate::oeb::polish::style::{ItemValue, Style, VerticalAlign};

use super::utils::{convert_color, int_or_zero};

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

/// The run-level CSS -> `w:rPr` property set -- port of `TextStyle`'s
/// `ALL_PROPS` fields (everything `__init__` computes from `css`).
/// Deliberately excludes `DOCXStyle`'s bookkeeping fields (`id`/
/// `name`/`next_style`) and the hash/serialize machinery, which need
/// `StylesManager`'s deduplication pass -- not ported yet.
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
