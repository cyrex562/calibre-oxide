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
}
