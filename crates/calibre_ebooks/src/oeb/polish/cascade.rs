//! Port of `old_src/src/calibre/ebooks/oeb/polish/cascade.py`.
//!
//! This file is, fundamentally, CSS cascade resolution: selector
//! matching, specificity ordering, `!important`, `@media`/`@import`
//! resolution, and inheritance, built on Python's `css_parser`
//! (stylesheet/rule/declaration parsing), `css_selectors` (selector
//! matching against the DOM) and `tinycss.fonts3` (`font`/`font-family`
//! shorthand parsing). None of those exist in this crate -- issues
//! #34/#35/#36 and #161's foundation PR already established there is no
//! general CSS parser or selector-matching engine here (see
//! `oeb::polish::utils::parse_css`'s docs and `oeb::stylizer`'s module
//! docs for the narrower, already-real alternative this crate has
//! instead).
//!
//! What *doesn't* need CSS parsing is ported for real:
//!
//! - [`INHERITED`] (the property-name data itself).
//! - [`Specificity`]/[`StyleDeclaration`]/[`PropertyValue`] (the value
//!   shapes `resolve_declarations` et al. operate on).
//! - [`resolve_declarations`]/[`resolve_pseudo_declarations`] (generic
//!   "pick the winning declaration per property, respecting
//!   `!important`" reduction over an already-computed list -- this is
//!   the actual cascade *algorithm*, independent of where the
//!   declarations came from).
//! - [`resolve_property`]/[`resolve_pseudo_property`] (ancestor-chain
//!   inheritance walk over a `style_map`, against
//!   [`crate::mobi::dom::Dom`] -- the real content-document tree type
//!   this crate uses elsewhere, standing in for Python's `lxml`
//!   elements).
//! - [`defvals`] (wraps [`crate::oeb::normalize_css::DEFAULTS`] as
//!   `PropertyValue`s).
//!
//! What genuinely can't be ported without a CSS parser and selector
//! engine -- `todo!()`, one per blocked capability:
//!
//! - [`html_css_stylesheet`]: needs [`super::utils::parse_css`].
//! - [`media_ok`]/[`media_allowed`]: needs a CSS3 media-query grammar
//!   parser (Python's `tinycss.mediaquery3.CSSMedia3Parser`) to handle
//!   negation/media-type/media-feature expressions -- this is itself a
//!   small CSS grammar, not just string matching, so it's the same kind
//!   of gap as `parse_css`, not a simplification of it.
//! - [`iterrules`]: needs `CSSRule`/`CSSStyleSheet` objects from a real
//!   parse.
//! - [`normalize_style_declaration`]/[`iterdeclaration`]: need
//!   `css_parser.Property` objects plus `tinycss.fonts3`'s
//!   `font-family` normalization.
//! - [`resolve_styles`]: the top-level orchestrator; needs all of the
//!   above plus `css_selectors.Select` selector matching.

use std::collections::HashMap;

use anyhow::Result;

use crate::mobi::dom::{Dom, NodeId};
use crate::oeb::normalize_css::DEFAULTS;

use super::container::Container;

/// Port of `calibre.ebooks.oeb.stylizer.INHERITED`: CSS properties that
/// inherit from an ancestor when not explicitly set.
pub const INHERITED: &[&str] = &[
    "azimuth",
    "border-collapse",
    "border-spacing",
    "caption-side",
    "color",
    "cursor",
    "direction",
    "elevation",
    "empty-cells",
    "font-family",
    "font-size",
    "font-style",
    "font-variant",
    "font-weight",
    "letter-spacing",
    "line-height",
    "list-style-image",
    "list-style-position",
    "list-style-type",
    "orphans",
    "page-break-inside",
    "pitch-range",
    "pitch",
    "quotes",
    "richness",
    "speak-header",
    "speak-numeral",
    "speak-punctuation",
    "speak",
    "speech-rate",
    "stress",
    "text-align",
    "text-indent",
    "text-transform",
    "visibility",
    "voice-family",
    "volume",
    "white-space",
    "widows",
    "word-spacing",
    "text-shadow",
];

/// Port of the `Values` tuple subclass: a resolved property value plus
/// which sheet it came from (for URL resolution) and whether it carried
/// `!important`. Where Python stores a tuple of parsed `css_parser`
/// `Value` objects, this stores the value's serialized CSS text -- the
/// only representation available without a real CSS value parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyValue {
    pub css_text: String,
    pub sheet_name: Option<String>,
    pub is_important: bool,
}

impl PropertyValue {
    pub fn new(
        css_text: impl Into<String>,
        sheet_name: Option<String>,
        is_important: bool,
    ) -> Self {
        Self {
            css_text: css_text.into(),
            sheet_name,
            is_important,
        }
    }
}

/// Port of the `Specificity` namedtuple: `(is_style, num_id, num_class,
/// num_elem, rule_index)`. Ordered exactly like the Python tuple (so
/// `Ord`/`sort` on a list of these matches
/// `x.sort(key=itemgetter(0), reverse=True)` against the `StyleDeclaration.index`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Specificity {
    pub is_style: bool,
    pub num_id: u32,
    pub num_class: u32,
    pub num_elem: u32,
    pub rule_index: u64,
}

/// Port of `specificity()`. Python computes `selector.specificity` via
/// `css_selectors`; here the caller supplies the already-known
/// `(num_id, num_class, num_elem)` triple (e.g. from a selector-matching
/// engine this crate doesn't have) -- this function only does the real,
/// parser-independent part: combining it with `rule_index`/`is_style`
/// into the ordering key.
pub fn specificity(
    rule_index: u64,
    sel_specificity: (u32, u32, u32),
    is_style: bool,
) -> Specificity {
    Specificity {
        is_style,
        num_id: sel_specificity.0,
        num_class: sel_specificity.1,
        num_elem: sel_specificity.2,
        rule_index,
    }
}

/// Port of the `StyleDeclaration` namedtuple.
#[derive(Debug, Clone)]
pub struct StyleDeclaration {
    pub index: Specificity,
    pub declaration: HashMap<String, PropertyValue>,
    pub pseudo_element: Option<String>,
}

/// Port of `resolve_declarations`: for each property name appearing in
/// any of `decls`, picks the winning value -- the first `!important`
/// value found, else the first value found. `decls` must already be in
/// descending-specificity order (Python sorts the caller's list with
/// `x.sort(key=itemgetter(0), reverse=True)` *before* calling this).
pub fn resolve_declarations(decls: &[StyleDeclaration]) -> HashMap<String, PropertyValue> {
    let mut property_names = std::collections::HashSet::new();
    for d in decls {
        property_names.extend(d.declaration.keys().cloned());
    }
    let mut ans = HashMap::new();
    for name in property_names {
        let mut first_val: Option<PropertyValue> = None;
        for decl in decls {
            if let Some(x) = decl.declaration.get(&name) {
                if x.is_important {
                    first_val = Some(x.clone());
                    break;
                }
                if first_val.is_none() {
                    first_val = Some(x.clone());
                }
            }
        }
        if let Some(v) = first_val {
            ans.insert(name, v);
        }
    }
    ans
}

/// Port of `resolve_pseudo_declarations`: groups `decls` by
/// `pseudo_element` and resolves each group independently.
pub fn resolve_pseudo_declarations(
    decls: &[StyleDeclaration],
) -> HashMap<Option<String>, HashMap<String, PropertyValue>> {
    let mut groups: HashMap<Option<String>, Vec<StyleDeclaration>> = HashMap::new();
    for d in decls {
        groups
            .entry(d.pseudo_element.clone())
            .or_default()
            .push(d.clone());
    }
    groups
        .into_iter()
        .map(|(k, v)| (k, resolve_declarations(&v)))
        .collect()
}

/// Port of `defvals()`: the CSS initial value of every property in
/// [`crate::oeb::normalize_css::DEFAULTS`], as a [`PropertyValue`].
pub fn defvals() -> HashMap<&'static str, PropertyValue> {
    DEFAULTS
        .iter()
        .map(|(&k, &v)| (k, PropertyValue::new(v, None, false)))
        .collect()
}

/// Port of `resolve_property`: walks up the ancestor chain (for
/// inheritable properties only) looking for a declared value in
/// `style_map`, falling back to the CSS initial value from
/// [`defvals`].
pub fn resolve_property(
    style_map: &HashMap<NodeId, HashMap<String, PropertyValue>>,
    dom: &Dom,
    elem: NodeId,
    name: &str,
) -> Option<PropertyValue> {
    let inheritable = INHERITED.contains(&name);
    let mut q = Some(elem);
    while let Some(node) = q {
        if let Some(s) = style_map.get(&node) {
            if let Some(val) = s.get(name) {
                return Some(val.clone());
            }
        }
        q = if inheritable { dom.parent(node) } else { None };
    }
    defvals().get(name).cloned()
}

/// Port of `resolve_pseudo_property`.
#[allow(clippy::too_many_arguments)]
pub fn resolve_pseudo_property(
    style_map: &HashMap<NodeId, HashMap<String, PropertyValue>>,
    pseudo_style_map: &HashMap<NodeId, HashMap<String, HashMap<String, PropertyValue>>>,
    dom: &Dom,
    elem: NodeId,
    prop: &str,
    name: &str,
    abort_on_missing: bool,
    check_if_pseudo_applies: bool,
    check_ancestors: bool,
) -> Option<PropertyValue> {
    if check_if_pseudo_applies {
        let mut q = Some(elem);
        while let Some(node) = q {
            let val = pseudo_style_map
                .get(&node)
                .and_then(|m| m.get(prop))
                .and_then(|m| m.get(name));
            if val.is_some() {
                return val.cloned();
            }
            if !check_ancestors {
                break;
            }
            q = dom.parent(node);
        }
        return None;
    }
    let sub_map = pseudo_style_map.get(&elem);
    if abort_on_missing && sub_map.is_none() {
        return None;
    }
    if let Some(sub_map) = sub_map {
        let prop_map = sub_map.get(prop);
        if abort_on_missing && prop_map.is_none() {
            return None;
        }
        if let Some(prop_map) = prop_map {
            if let Some(val) = prop_map.get(name) {
                return Some(val.clone());
            }
        }
    }
    if INHERITED.contains(&name) {
        if check_ancestors {
            let mut q = dom.parent(elem);
            while let Some(node) = q {
                let val = pseudo_style_map
                    .get(&node)
                    .and_then(|m| m.get(prop))
                    .and_then(|m| m.get(name));
                if val.is_some() {
                    return val.cloned();
                }
                q = dom.parent(node);
            }
        }
        return resolve_property(style_map, dom, elem, name);
    }
    defvals().get(name).cloned()
}

// -- genuinely blocked: needs a real CSS parser/selector engine --------

/// Port of `html_css_stylesheet`: the built-in user-agent stylesheet
/// (`templates/html.css`), parsed once and cached. Needs
/// [`super::utils::parse_css`].
pub fn html_css_stylesheet(_container: &mut Container) -> Result<()> {
    todo!(
        "placeholder: needs a real CSS parser to parse templates/html.css \
         into a stylesheet object -- see oeb::polish::utils::parse_css's docs \
         (same gap as issues #34/#35/#36)"
    )
}

/// Port of `media_ok`. Needs a CSS3 media-query grammar parser (Python's
/// `tinycss.mediaquery3.CSSMedia3Parser`) to evaluate negation,
/// media-type and media-feature expressions -- a small CSS grammar in
/// its own right, not something reducible to plain string matching.
pub fn media_ok(_media_text: &str) -> bool {
    todo!(
        "placeholder: evaluating a CSS3 media query needs a real media-query \
         parser (Python's tinycss.mediaquery3.CSSMedia3Parser), which this \
         crate doesn't have -- same category of gap as parse_css"
    )
}

/// Port of `media_allowed`. Needs a parsed `@media` rule's `mediaText`.
pub fn media_allowed(_media_text: Option<&str>) -> bool {
    todo!("placeholder: needs media_ok, see its docs")
}

/// Port of `iterrules`. Needs `CSSRule`/`CSSStyleSheet` objects from a
/// real CSS parse (import/media rule resolution).
pub fn iterrules(_container: &mut Container, _sheet_name: &str) -> Result<Vec<()>> {
    todo!(
        "placeholder: iterating CSS rules (resolving @import/@media) needs a \
         real CSS parser producing CSSRule/CSSStyleSheet objects -- see \
         oeb::polish::utils::parse_css's docs"
    )
}

/// Port of `normalize_style_declaration`/`iterdeclaration`. Needs
/// `css_parser.Property` objects (from a real CSS declaration parse)
/// plus `tinycss.fonts3`'s `font-family` shorthand normalization.
pub fn normalize_style_declaration(
    _decl_css_text: &str,
    _sheet_name: &str,
) -> HashMap<String, PropertyValue> {
    todo!(
        "placeholder: needs a real CSS declaration parser (css_parser.Property) \
         plus tinycss.fonts3's font-family normalization -- see \
         oeb::polish::utils::parse_css's docs"
    )
}

/// Port of `resolve_styles`: the top-level cascade resolver. Needs
/// everything above (real parsing) plus `css_selectors.Select` selector
/// matching against the DOM, which this crate also does not have.
pub fn resolve_styles(_container: &mut Container, _name: &str) -> Result<()> {
    todo!(
        "placeholder: full cascade resolution needs a real CSS parser AND a \
         CSS selector-matching engine (Python's css_selectors.Select), \
         neither of which exists in this crate -- see this module's docs for \
         which pieces (resolve_declarations, resolve_property, INHERITED, ...) \
         *are* ported for real"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pv(text: &str, important: bool) -> PropertyValue {
        PropertyValue::new(text, None, important)
    }

    #[test]
    fn resolve_declarations_prefers_important_over_earlier_specificity() {
        let mut low = HashMap::new();
        low.insert("color".to_string(), pv("red", false));
        let mut high_important = HashMap::new();
        high_important.insert("color".to_string(), pv("blue", true));
        let decls = vec![
            StyleDeclaration {
                index: Specificity {
                    is_style: false,
                    num_id: 0,
                    num_class: 1,
                    num_elem: 0,
                    rule_index: 5,
                },
                declaration: low,
                pseudo_element: None,
            },
            StyleDeclaration {
                index: Specificity {
                    is_style: false,
                    num_id: 0,
                    num_class: 0,
                    num_elem: 1,
                    rule_index: 1,
                },
                declaration: high_important,
                pseudo_element: None,
            },
        ];
        let resolved = resolve_declarations(&decls);
        assert_eq!(resolved.get("color"), Some(&pv("blue", true)));
    }

    #[test]
    fn resolve_declarations_takes_first_when_no_important() {
        let mut a = HashMap::new();
        a.insert("color".to_string(), pv("red", false));
        let mut b = HashMap::new();
        b.insert("color".to_string(), pv("blue", false));
        let decls = vec![
            StyleDeclaration {
                index: Specificity {
                    is_style: true,
                    num_id: 0,
                    num_class: 0,
                    num_elem: 0,
                    rule_index: 0,
                },
                declaration: a,
                pseudo_element: None,
            },
            StyleDeclaration {
                index: Specificity {
                    is_style: false,
                    num_id: 1,
                    num_class: 0,
                    num_elem: 0,
                    rule_index: 1,
                },
                declaration: b,
                pseudo_element: None,
            },
        ];
        let resolved = resolve_declarations(&decls);
        assert_eq!(resolved.get("color"), Some(&pv("red", false)));
    }

    #[test]
    fn specificity_orders_like_the_python_tuple() {
        let a = specificity(0, (1, 0, 0), false);
        let b = specificity(0, (0, 5, 5), false);
        assert!(
            a > b,
            "an id selector must outrank any number of classes/elements"
        );
    }

    #[test]
    fn resolve_property_walks_ancestors_for_inherited_properties() {
        let dom = Dom::parse("<html><body><div><p>x</p></div></body></html>");
        let p = dom.find_first_tag_global("p").unwrap();
        let div = dom.find_first_tag_global("div").unwrap();
        let mut style_map = HashMap::new();
        let mut div_style = HashMap::new();
        div_style.insert("color".to_string(), pv("green", false));
        style_map.insert(div, div_style);
        let val = resolve_property(&style_map, &dom, p, "color");
        assert_eq!(val, Some(pv("green", false)));
    }

    #[test]
    fn resolve_property_does_not_inherit_non_inherited_properties() {
        let dom = Dom::parse("<html><body><div><p>x</p></div></body></html>");
        let p = dom.find_first_tag_global("p").unwrap();
        let div = dom.find_first_tag_global("div").unwrap();
        let mut style_map = HashMap::new();
        let mut div_style = HashMap::new();
        div_style.insert("display".to_string(), pv("none", false));
        style_map.insert(div, div_style);
        // display is not inherited: p should fall back to the CSS default.
        let val = resolve_property(&style_map, &dom, p, "display");
        assert_eq!(val.map(|v| v.css_text), Some("inline".to_string()));
    }

    #[test]
    fn inherited_set_contains_the_expected_properties() {
        assert!(INHERITED.contains(&"color"));
        assert!(INHERITED.contains(&"font-family"));
        assert!(!INHERITED.contains(&"display"));
        assert!(!INHERITED.contains(&"margin-top"));
    }
}
