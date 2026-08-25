//! Per-element resolved-style view over [`super::cascade`]'s output --
//! the accessor half of `calibre.ebooks.oeb.stylizer.Style`.
//!
//! `stylizer.py`'s `Stylizer` does two jobs Python bundles into one
//! class: (1) parse every linked/embedded stylesheet and match
//! selectors against the tree, and (2) offer a per-element `Style`
//! object with dict-like property access plus a handful of derived
//! accessors (`color`, `fontSize`, `effective_text_decoration`, ...).
//! Job (1) is [`super::cascade::resolve_styles`], ported in full for
//! issue #164 -- a real CSS tokenizer/selector-matcher/cascade, not a
//! stub. Job (2) is this module: issue #132 flagged `docx/writer`'s
//! remaining files (issue #23) as blocked on "a real OEB stylizer",
//! but that premise was already stale by the time it was filed three
//! days later -- [`super::cascade`] existed the whole time, just for
//! a different immediate consumer ([`super::css`]/[`super::stats`]).
//! This is the sixth time this session a "blocked on X" claim turned
//! out to only be missing the adapter, not X itself.
//!
//! [`Style`] wraps a [`super::cascade::ResolvedStyles`] (already fully
//! resolved -- selectors matched, specificity applied) plus a
//! [`Profile`] (the handful of `calibre.customize.profiles.Plugin`
//! constants `fontSize`'s keyword/relative-size handling needs) and
//! exposes exactly the surface `docx/writer/styles.py`/`from_html.py`
//! actually call: [`Style::get`] (`Style._get`), [`Style::own`]
//! (`Style._style.get`, no inheritance), [`Style::item`]
//! (`Style.__getitem__`'s fallback branch for properties with no
//! dedicated accessor), and the dedicated accessors themselves
//! (`color`, `background_color`, `font_size`, `line_height`,
//! `effective_text_decoration`, `first_vertical_align`, `is_hidden`,
//! `width`/`height`, margins/paddings).
//!
//! Unlike Python, nothing here caches a computed value on `self` --
//! `Style` is a lightweight `Copy` view (a few references plus a
//! `NodeId`), cheap enough to recompute per call. Python's own
//! per-property `self._fontSize`/`self._width`/... caches exist
//! because computing them walks the ancestor chain; this port accepts
//! that cost rather than inventing a cache invalidation story no
//! caller has asked for yet.

use crate::dom::{Dom, NodeId};
use crate::oeb::transforms::flatcss::unit_convert;

use super::cascade::{self, ResolvedStyles};

/// The `calibre.customize.profiles.Plugin`/`OutputProfile` fields
/// [`Style::font_size`]/[`Style::width`]/[`Style::height`] read --
/// not the full profile-plugin system (per-device screen sizes,
/// themes, ...), just the "default" profile's real constants, which
/// is what `Stylizer(profile=None)` resolves to in practice (Python's
/// own comment: "Use the default profile... doing so might well have
/// hard to debug font size effects", i.e. deliberately not
/// `opts.output_profile`).
///
/// Port of `calibre.customize.profiles.Plugin`'s class-level defaults
/// (`fbase`/`fsizes`/`screen_size`/`dpi`) plus `Plugin.__init__`'s
/// `width_pts`/`height_pts` derivation.
#[derive(Debug, Clone)]
pub struct Profile {
    pub dpi: f64,
    pub fbase: f64,
    /// `(keyword, css2-index, points)` triples, port of
    /// `Plugin.__init__`'s `self.fsizes` after zipping the raw size
    /// list with `FONT_SIZES`. `x-small` and the implicit "7th" entry
    /// each have one side `None` (`x-small` has no CSS2 index; the
    /// last entry has no keyword) -- reproduced as-is.
    pub fsizes: Vec<(Option<&'static str>, Option<i32>, f64)>,
    pub width_pts: f64,
    pub height_pts: f64,
}

impl Default for Profile {
    fn default() -> Self {
        let dpi = 100.0;
        let (width, height) = (1600.0, 1200.0);
        const FONT_SIZES: [(Option<&str>, Option<i32>); 8] = [
            (Some("xx-small"), Some(1)),
            (Some("x-small"), None),
            (Some("small"), Some(2)),
            (Some("medium"), Some(3)),
            (Some("large"), Some(4)),
            (Some("x-large"), Some(5)),
            (Some("xx-large"), Some(6)),
            (None, Some(7)),
        ];
        let raw_fsizes = [5.0, 7.0, 9.0, 12.0, 13.5, 17.0, 20.0, 22.0, 24.0];
        let fsizes = FONT_SIZES
            .iter()
            .zip(raw_fsizes.iter())
            .map(|(&(name, num), &sz)| (name, num, sz))
            .collect();
        Profile {
            dpi,
            fbase: 12.0,
            fsizes,
            width_pts: width * 72.0 / dpi,
            height_pts: height * 72.0 / dpi,
        }
    }
}

impl Profile {
    /// Port of `Plugin.__init__`'s `self.fnames` -- a keyword's point
    /// size, skipping the one `fsizes` entry with no keyword.
    pub fn fname(&self, keyword: &str) -> Option<f64> {
        self.fsizes
            .iter()
            .find(|&&(name, _, _)| name == Some(keyword))
            .map(|&(_, _, sz)| sz)
    }
}

/// `Style.__getitem__`'s return: Python's dynamic typing lets it
/// return either the unit-converted number or the original string
/// unchanged (`_unit_convert` returns `value` itself when nothing
/// matches the length grammar); this is that union, made explicit.
#[derive(Debug, Clone, PartialEq)]
pub enum ItemValue {
    Number(f64),
    Text(String),
}

impl ItemValue {
    pub fn as_number(&self) -> Option<f64> {
        match self {
            ItemValue::Number(n) => Some(*n),
            ItemValue::Text(_) => None,
        }
    }
}

/// A vertical-align value: [`Style::first_vertical_align`] can return
/// either a resolved length (a number of points) or a keyword.
#[derive(Debug, Clone, PartialEq)]
pub enum VerticalAlign {
    Points(f64),
    Keyword(String),
}

/// The resolved style of one element -- the accessor half of Python's
/// `Style`. See the module docs.
#[derive(Clone, Copy)]
pub struct Style<'a> {
    pub node: NodeId,
    dom: &'a Dom,
    resolved: &'a ResolvedStyles,
    profile: &'a Profile,
    /// `self._stylizer.body_font_size` -- the root element's font
    /// size, backing `rem`. Python recomputes this once per
    /// `Stylizer` (`self.body_font_size = self.profile.fbase`, never
    /// updated even if the body's own `font-size` differs) -- ported
    /// as-is, a plain constant rather than the body element's actual
    /// resolved size.
    body_font_size: f64,
}

impl<'a> Style<'a> {
    pub fn new(
        dom: &'a Dom,
        resolved: &'a ResolvedStyles,
        profile: &'a Profile,
        node: NodeId,
    ) -> Self {
        Style {
            node,
            dom,
            resolved,
            profile,
            body_font_size: profile.fbase,
        }
    }

    fn at(&self, node: NodeId) -> Self {
        Style { node, ..*self }
    }

    /// Port of `Style._get_parent`/`_has_parent`. lxml's root element
    /// has no parent at all (`getparent()` returns `None`); `Dom`
    /// instead nests every element under a non-element document node,
    /// so climbing past the last real element must stop there too, or
    /// every "no parent" case in Python (e.g. `lineHeight`'s root
    /// fallback) would incorrectly keep climbing here.
    fn parent(&self) -> Option<Self> {
        let p = self.dom.parent(self.node)?;
        self.dom.tag(p)?;
        Some(self.at(p))
    }

    /// Port of `Style._get`: the specified value, walking ancestors
    /// only for an inheritable property, else the CSS initial value.
    /// [`super::cascade::resolve_property`] already implements exactly
    /// this walk.
    pub fn get(&self, name: &str) -> String {
        cascade::resolve_property(&self.resolved.style_map, self.dom, self.node, name)
            .map(|v| v.css_text)
            .unwrap_or_default()
    }

    /// Port of `Style.get`/`Style._style.get`: this element's own
    /// declared value, with no inheritance and no default fallback.
    pub fn own(&self, name: &str) -> Option<String> {
        self.resolved
            .style_map
            .get(&self.node)
            .and_then(|m| m.get(name))
            .map(|v| v.css_text.clone())
    }

    /// Port of `Style._unit_convert`. `base` defaults to
    /// [`Style::width`], `font` to [`Style::font_size`] -- matching
    /// Python's own `base=None -> self.width`/`font=None -> self.fontSize`
    /// defaulting (`0` is a valid explicit `font`, distinct from
    /// "unset", exactly like Python's `if not font and font != 0`).
    pub fn unit_convert(&self, value: &str, base: Option<f64>, font: Option<f64>) -> Option<f64> {
        let base = base.unwrap_or_else(|| self.width());
        let font = font.unwrap_or_else(|| self.font_size());
        unit_convert(value, base, font, self.profile.dpi, self.body_font_size)
    }

    /// Port of `Style.__getitem__`'s fallback branch: what a property
    /// with no dedicated Rust/Python accessor resolves to -- the
    /// specified-or-default value, unit-converted to a number when
    /// it's a length, else the raw text.
    pub fn item(&self, name: &str) -> ItemValue {
        let raw = self.get(name);
        match self.unit_convert(&raw, None, None) {
            Some(n) => ItemValue::Number(n),
            None => ItemValue::Text(raw),
        }
    }

    /// Port of `Style.color`.
    pub fn color(&self) -> String {
        let val = self.get("color");
        if !val.is_empty() && validate_color(&val) {
            val
        } else {
            DEFAULT_COLOR.to_string()
        }
    }

    /// Port of `Style.backgroundColor`. Only `background-color`/the
    /// `background` shorthand's own color token are read --
    /// inheritance/defaults are deliberately not used, matching
    /// Python's own docstring. `None` when no color is set (Python's
    /// `False`-then-`None` sentinel dance collapses to a plain
    /// `Option` here).
    pub fn background_color(&self) -> Option<String> {
        if let Some(val) = self.own("background-color") {
            if validate_color(&val) {
                return Some(val);
            }
        }
        if let Some(shorthand) = self.own("background") {
            if let Some(color) = extract_color_token(&shorthand) {
                return Some(color);
            }
        }
        None
    }

    /// Port of `Style.fontSize`.
    pub fn font_size(&self) -> f64 {
        let base = match self.parent() {
            Some(p) => p.font_size(),
            None => self.profile.fbase,
        };
        match self.own("font-size") {
            Some(size) => self.normalize_font_size(&size, base),
            None => base,
        }
    }

    fn normalize_font_size(&self, value: &str, base: f64) -> f64 {
        let value = value.replace(['"', '\''], "");
        let value = if value == "inherit" {
            return base;
        } else {
            value
        };
        if let Some(sz) = self.profile.fname(&value) {
            return sz;
        }
        if value == "smaller" {
            let mut result = None;
            for &(_, _, size) in &self.profile.fsizes {
                if base <= size {
                    break;
                }
                result = Some(size);
            }
            return match result {
                Some(r) => r,
                None => base * (1.0 / 1.2),
            };
        }
        if value == "larger" {
            let mut result = None;
            for &(_, _, size) in self.profile.fsizes.iter().rev() {
                if base >= size {
                    break;
                }
                result = Some(size);
            }
            return match result {
                Some(r) => r,
                None => base * 1.2,
            };
        }
        match self.unit_convert(&value, Some(base), Some(base)) {
            Some(n) if n >= 0.0 => n,
            Some(_) => self.normalize_font_size("smaller", base),
            None => base,
        }
    }

    /// Port of `Style.width`. Reads the element's own `width`
    /// attribute or `width`/`max-width` declarations; `auto`/unset
    /// falls back to the parent's width, or the profile's screen
    /// width at the root.
    pub fn width(&self) -> f64 {
        let base = match self.parent() {
            Some(p) => p.width(),
            None => self.profile.width_pts,
        };
        let width = self
            .dom
            .node(self.node)
            .attrs
            .get("width")
            .cloned()
            .or_else(|| self.own("width"));
        let mut result = match width.as_deref() {
            None | Some("") | Some("auto") => base,
            Some(w) => self.unit_convert(w, Some(base), None).unwrap_or(base),
        };
        if let Some(max_width) = self.own("max-width") {
            if let Some(mw) = self.unit_convert(&max_width, Some(base), None) {
                result = result.min(mw);
            }
        }
        result
    }

    /// Port of `Style.parent_width`.
    pub fn parent_width(&self) -> f64 {
        match self.parent() {
            Some(p) => p.width(),
            None => self.width(),
        }
    }

    /// Port of `Style.height`.
    pub fn height(&self) -> f64 {
        let base = match self.parent() {
            Some(p) => p.height(),
            None => self.profile.height_pts,
        };
        let height = self
            .dom
            .node(self.node)
            .attrs
            .get("height")
            .cloned()
            .or_else(|| self.own("height"));
        let mut result = match height.as_deref() {
            None | Some("") | Some("auto") => base,
            Some(h) => self.unit_convert(h, Some(base), None).unwrap_or(base),
        };
        if let Some(max_height) = self.own("max-height") {
            if let Some(mh) = self.unit_convert(&max_height, Some(base), None) {
                result = result.min(mh);
            }
        }
        result
    }

    /// Port of `Style.lineHeight`.
    pub fn line_height(&self) -> f64 {
        if let Some(lineh) = self.own("line-height") {
            let lineh = if lineh == "normal" {
                "1.2".to_string()
            } else {
                lineh
            };
            if let Ok(factor) = lineh.parse::<f64>() {
                return factor * self.font_size();
            }
            let font_size = self.font_size();
            return self
                .unit_convert(&lineh, None, Some(font_size))
                .unwrap_or(1.2 * font_size);
        }
        if let Some(p) = self.parent() {
            return p.line_height();
        }
        1.2 * self.font_size()
    }

    /// Port of `Style.effective_text_decoration`. Only the element's
    /// own and its immediate parent's *directly declared*
    /// `text-decoration` are consulted (`self._style`/`parent._style`,
    /// not the full inherited/default chain) -- a deliberate
    /// simplification of the real CSS containing-block algorithm, per
    /// Python's own docstring.
    pub fn effective_text_decoration(&self) -> Option<String> {
        let css = self.own("text-decoration");
        let pcss = self.parent().and_then(|p| p.own("text-decoration"));
        let css_is_none_ish = matches!(css.as_deref(), None | Some("none") | Some("inherit"));
        let pcss_is_real = !matches!(pcss.as_deref(), None | Some("none"));
        if css_is_none_ish && pcss_is_real {
            return pcss;
        }
        css
    }

    /// Port of `Style.first_vertical_align`.
    pub fn first_vertical_align(&self) -> Option<VerticalAlign> {
        let val = self.item("vertical-align");
        let is_baseline = matches!(&val, ItemValue::Text(t) if t == "baseline");
        if !is_baseline {
            let raw_val = self.get("vertical-align");
            if raw_val.contains('%') {
                if let Some(n) = self.unit_convert(&raw_val, Some(self.line_height()), None) {
                    return Some(VerticalAlign::Points(n));
                }
            }
            return Some(match val {
                ItemValue::Number(n) => VerticalAlign::Points(n),
                ItemValue::Text(t) => VerticalAlign::Keyword(t),
            });
        }
        let parent = self.parent()?;
        if parent.get("display").contains("inline") {
            return parent.first_vertical_align();
        }
        None
    }

    /// Port of `Style.marginTop`/`marginBottom`/`marginLeft`/`marginRight`.
    pub fn margin_top(&self) -> f64 {
        self.unit_convert(&self.get("margin-top"), Some(self.parent_width()), None)
            .unwrap_or(0.0)
    }
    pub fn margin_bottom(&self) -> f64 {
        self.unit_convert(&self.get("margin-bottom"), Some(self.parent_width()), None)
            .unwrap_or(0.0)
    }
    pub fn margin_left(&self) -> f64 {
        self.unit_convert(&self.get("margin-left"), Some(self.parent_width()), None)
            .unwrap_or(0.0)
    }
    pub fn margin_right(&self) -> f64 {
        self.unit_convert(&self.get("margin-right"), Some(self.parent_width()), None)
            .unwrap_or(0.0)
    }

    /// Port of `Style.paddingTop`/`paddingBottom`/`paddingLeft`/`paddingRight`.
    pub fn padding_top(&self) -> f64 {
        self.unit_convert(&self.get("padding-top"), Some(self.parent_width()), None)
            .unwrap_or(0.0)
    }
    pub fn padding_bottom(&self) -> f64 {
        self.unit_convert(&self.get("padding-bottom"), Some(self.parent_width()), None)
            .unwrap_or(0.0)
    }
    pub fn padding_left(&self) -> f64 {
        self.unit_convert(&self.get("padding-left"), Some(self.parent_width()), None)
            .unwrap_or(0.0)
    }
    pub fn padding_right(&self) -> f64 {
        self.unit_convert(&self.get("padding-right"), Some(self.parent_width()), None)
            .unwrap_or(0.0)
    }

    /// Port of `Style.is_hidden`.
    pub fn is_hidden(&self) -> bool {
        self.own("display").as_deref() == Some("none")
            || self.own("visibility").as_deref() == Some("hidden")
    }
}

const DEFAULT_COLOR: &str = "black";

/// Port of `stylizer.validate_color`, simplified: this crate has no
/// port of `css_parser`'s full CSS Level 2 color-profile grammar
/// validator, so this checks the common real-world forms instead --
/// `#rgb`/`#rrggbb`(/`#rrggbbaa`), `rgb()`/`rgba()`/`hsl()`/`hsla()`
/// function calls, `transparent`/`currentcolor`, and the CSS named-color
/// keywords. A disclosed simplification, not a 1:1 grammar port --
/// same precedent as this session's other "port the intent, not an
/// unported dependency" calls.
pub fn validate_color(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    if let Some(hex) = value.strip_prefix('#') {
        return matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit());
    }
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("rgb(")
        || lower.starts_with("rgba(")
        || lower.starts_with("hsl(")
        || lower.starts_with("hsla(")
    {
        return value.ends_with(')');
    }
    if lower == "transparent" || lower == "currentcolor" || lower == "inherit" {
        return true;
    }
    CSS_NAMED_COLORS.contains(&lower.as_str())
}

/// Scans a `background` shorthand value for the first token that
/// looks like a valid color, matching what Python's real parse (via
/// `css_parser`, which fully tokenizes the shorthand and inspects
/// `propertyValue`) would find in the common case -- a color keyword
/// or function call among space-separated tokens. Doesn't handle a
/// color embedded inside another function's arguments (not a shape
/// `background` shorthand values take in practice).
fn extract_color_token(shorthand: &str) -> Option<String> {
    for token in shorthand.split_whitespace() {
        let token = token.trim_end_matches(',');
        if validate_color(token) {
            return Some(token.to_string());
        }
    }
    None
}

/// The CSS Level 2 + common CSS3 extended named colors. Not the full
/// ~147-name CSS3 list -- the ones a real document is likely to use --
/// disclosed alongside [`validate_color`]'s own simplification note.
const CSS_NAMED_COLORS: &[&str] = &[
    "black",
    "silver",
    "gray",
    "grey",
    "white",
    "maroon",
    "red",
    "purple",
    "fuchsia",
    "green",
    "lime",
    "olive",
    "yellow",
    "navy",
    "blue",
    "teal",
    "aqua",
    "orange",
    "pink",
    "brown",
    "gold",
    "indigo",
    "violet",
    "coral",
    "salmon",
    "khaki",
    "crimson",
    "chocolate",
    "tan",
    "beige",
    "azure",
    "ivory",
    "lavender",
    "plum",
    "orchid",
    "turquoise",
    "wheat",
    "skyblue",
    "steelblue",
    "slategray",
    "slategrey",
    "darkred",
    "darkblue",
    "darkgreen",
    "darkorange",
    "lightgray",
    "lightgrey",
    "lightblue",
    "lightgreen",
    "lightyellow",
    "lightpink",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::polish::cascade::{PropertyValue, ResolvedStyles};
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
    fn get_falls_back_to_the_default_for_an_unspecified_property() {
        let dom = make("<html><body><p>x</p></body></html>");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let p = find(&dom, "p");
        let style = Style::new(&dom, &resolved, &profile, p);
        assert_eq!(style.get("display"), "inline");
    }

    #[test]
    fn get_inherits_from_an_ancestor_for_an_inheritable_property() {
        let dom = make("<html><body><p><span>x</span></p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(p, &[("color", "red")])]);
        let profile = Profile::default();
        let span = find(&dom, "span");
        let style = Style::new(&dom, &resolved, &profile, span);
        assert_eq!(style.get("color"), "red");
    }

    #[test]
    fn get_does_not_inherit_a_non_inherited_property() {
        let dom = make("<html><body><p><span>x</span></p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(p, &[("display", "block")])]);
        let profile = Profile::default();
        let span = find(&dom, "span");
        let style = Style::new(&dom, &resolved, &profile, span);
        assert_eq!(style.get("display"), "inline");
    }

    #[test]
    fn color_falls_back_to_black_when_unset_or_invalid() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(p, &[("color", "not-a-color")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        assert_eq!(style.color(), "black");
    }

    #[test]
    fn color_returns_a_valid_declared_value() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(p, &[("color", "#ff0000")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        assert_eq!(style.color(), "#ff0000");
    }

    #[test]
    fn background_color_reads_the_shorthand_when_the_longhand_is_absent() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(p, &[("background", "url(x.png) no-repeat blue")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        assert_eq!(style.background_color().as_deref(), Some("blue"));
    }

    #[test]
    fn background_color_is_none_when_nothing_is_set() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        assert_eq!(style.background_color(), None);
    }

    #[test]
    fn font_size_resolves_a_keyword_against_the_profile_table() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        // The default profile's fsizes table is [5, 7, 9, 12, 13.5, 17,
        // 20, 22, 24] zipped against [xx-small, x-small, small, medium,
        // large, x-large, xx-large, <unnamed>] -- "large" is index 4:
        // 13.5pt, not the 9-entry table's own 5th *value* (17).
        let resolved = resolved_with(&[(p, &[("font-size", "large")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        assert_eq!(style.font_size(), 13.5);
    }

    #[test]
    fn font_size_em_is_relative_to_the_parent() {
        let dom = make("<html><body><p><span>x</span></p></body></html>");
        let p = find(&dom, "p");
        let span = find(&dom, "span");
        let resolved = resolved_with(&[
            (p, &[("font-size", "20pt")]),
            (span, &[("font-size", "2em")]),
        ]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, span);
        assert_eq!(style.font_size(), 40.0);
    }

    #[test]
    fn font_size_defaults_to_the_profile_base_at_the_root() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        assert_eq!(style.font_size(), profile.fbase);
    }

    #[test]
    fn line_height_defaults_to_1_2_times_the_font_size_at_the_root() {
        let dom = make("<html></html>");
        let root = find(&dom, "html");
        let resolved = resolved_with(&[(root, &[("font-size", "10pt")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, root);
        assert_eq!(style.line_height(), 12.0);
    }

    #[test]
    fn line_height_with_no_own_declaration_defers_to_the_parent_not_its_own_font_size() {
        // A real, disclosed Python quirk (`# TODO: proper inheritance`
        // in `Style.lineHeight`): an element with no own `line-height`
        // but *with* a parent uses the parent's `lineHeight`
        // wholesale, ignoring its own `fontSize` entirely -- even
        // though its own font-size differs from the root's.
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(p, &[("font-size", "10pt")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        assert_eq!(style.line_height(), 1.2 * profile.fbase);
    }

    #[test]
    fn line_height_a_bare_number_is_a_multiplier() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(p, &[("font-size", "10pt"), ("line-height", "1.5")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        assert_eq!(style.line_height(), 15.0);
    }

    #[test]
    fn effective_text_decoration_inherits_from_the_containing_block_when_unset() {
        let dom = make("<html><body><p><span>x</span></p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(p, &[("text-decoration", "underline")])]);
        let profile = Profile::default();
        let span = find(&dom, "span");
        let style = Style::new(&dom, &resolved, &profile, span);
        assert_eq!(
            style.effective_text_decoration().as_deref(),
            Some("underline")
        );
    }

    #[test]
    fn effective_text_decoration_own_value_wins_over_the_parent() {
        let dom = make("<html><body><p><span>x</span></p></body></html>");
        let p = find(&dom, "p");
        let span = find(&dom, "span");
        let resolved = resolved_with(&[
            (p, &[("text-decoration", "underline")]),
            (span, &[("text-decoration", "line-through")]),
        ]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, span);
        assert_eq!(
            style.effective_text_decoration().as_deref(),
            Some("line-through")
        );
    }

    #[test]
    fn first_vertical_align_reads_a_keyword() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(p, &[("vertical-align", "top")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        assert_eq!(
            style.first_vertical_align(),
            Some(VerticalAlign::Keyword("top".to_string()))
        );
    }

    #[test]
    fn first_vertical_align_climbs_an_inline_parent_when_baseline() {
        let dom = make("<html><body><span><span>x</span></span></body></html>");
        let elements: Vec<NodeId> = dom
            .preorder_elements(dom.root)
            .into_iter()
            .filter(|&id| dom.tag(id) == Some("span"))
            .collect();
        let outer = elements[0];
        let inner = elements[1];
        let resolved = resolved_with(&[
            (outer, &[("display", "inline"), ("vertical-align", "super")]),
            (inner, &[("display", "inline")]),
        ]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, inner);
        assert_eq!(
            style.first_vertical_align(),
            Some(VerticalAlign::Keyword("super".to_string()))
        );
    }

    #[test]
    fn is_hidden_checks_display_and_visibility() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(p, &[("display", "none")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        assert!(style.is_hidden());
    }

    #[test]
    fn margins_convert_to_points_relative_to_the_parent_width() {
        let dom = make("<html><body><p><span>x</span></p></body></html>");
        let span = find(&dom, "span");
        let resolved = resolved_with(&[(span, &[("margin-left", "1in")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, span);
        assert_eq!(style.margin_left(), 72.0);
    }

    #[test]
    fn validate_color_accepts_hex_and_named_and_rejects_garbage() {
        assert!(validate_color("#fff"));
        assert!(validate_color("#ff0000"));
        assert!(validate_color("red"));
        assert!(validate_color("rgb(1,2,3)"));
        assert!(!validate_color("not-a-color"));
        assert!(!validate_color(""));
    }

    #[test]
    fn width_defaults_to_the_profile_screen_width_at_the_root() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        assert_eq!(style.width(), profile.width_pts);
    }
}
