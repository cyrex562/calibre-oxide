//! Port of `old_src/src/calibre/ebooks/oeb/polish/cascade.py`.
//!
//! This file is, fundamentally, CSS cascade resolution: selector
//! matching, specificity ordering, `!important`, `@media`/`@import`
//! resolution, and inheritance, built on Python's `css_parser`
//! (stylesheet/rule/declaration parsing), `css_selectors` (selector
//! matching against the DOM) and `tinycss.fonts3` (`font`/`font-family`
//! shorthand parsing). Issue #164 added real equivalents for all three
//! ([`crate::css`], and [`crate::oeb::fonts3`] for the narrow
//! `font-family` piece), closing every `todo!()` this file previously
//! had -- see each function's docs below for exactly what it does (and,
//! for a couple, the narrower-than-Python scope it settles for instead
//! of a `todo!()`).
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
//! - [`html_css_stylesheet`]: parses the bundled user-agent stylesheet
//!   (`templates/html.css`, embedded via `include_str!`) once, cached.
//! - [`media_ok`]/[`media_allowed`]: a scoped media-query-list parser
//!   (comma-separated `[not|only] <type> [and (<feature>[: <value>])]*`)
//!   -- real, but narrower than `tinycss.mediaquery3.CSSMedia3Parser`:
//!   feature *values* are never evaluated (matching Python's own
//!   `IGNORED_MEDIA_FEATURES` table, which always fails a query on any
//!   device-specific feature regardless of its value), and an
//!   unparseable query string is treated as "allowed" (matching
//!   Python's `except Exception: return True`).
//! - [`iterrules`]: walks `@import`/`@media` for real against
//!   [`crate::css::Stylesheet`].
//! - [`normalize_style_declaration`]/[`iterdeclaration`]: real, with one
//!   documented narrowing -- see [`iterdeclaration`]'s docs.
//! - [`resolve_styles`]: the top-level orchestrator, real end to end.

use std::collections::HashMap;
use std::sync::OnceLock;

use anyhow::Result;

use crate::css::{Rule, RuleType, Select, SelectorList, Stylesheet};
use crate::mobi::dom::{Dom, NodeId};
use crate::oeb::fonts3::{parse_font_family, serialize_font_family};
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

// -- now real, built on `crate::css` -----------------------------------

/// The bundled user-agent stylesheet's source text (`templates/html.css`
/// from calibre's own resources, copied into this crate -- see
/// [`html_css_stylesheet`]'s docs).
const HTML_CSS: &str = include_str!("../../../resources/templates/html.css");

/// Port of `html_css_stylesheet`: the built-in user-agent stylesheet,
/// parsed once and cached (Python caches it in a module-level global;
/// this uses [`OnceLock`] for the same effect). Takes no `Container`
/// parameter -- Python's only used it to call `container.parse_css`,
/// which needed no state from the container itself; parsing bundled,
/// static text needs none either.
pub fn html_css_stylesheet() -> &'static Stylesheet {
    static SHEET: OnceLock<Stylesheet> = OnceLock::new();
    SHEET.get_or_init(|| Stylesheet::parse(HTML_CSS))
}

/// Port of `stylizer.ALLOWED_MEDIA_TYPES`.
const ALLOWED_MEDIA_TYPES: &[&str] = &["screen", "all", "aural", "amzn-kf8"];

/// Port of `stylizer.IGNORED_MEDIA_FEATURES`: device-specific media
/// features that always fail a query, regardless of the value tested
/// against them (this port never evaluates the value -- see
/// [`media_ok`]'s docs).
const IGNORED_MEDIA_FEATURES: &[&str] = &[
    "width",
    "min-width",
    "max-width",
    "height",
    "min-height",
    "max-height",
    "device-width",
    "min-device-width",
    "max-device-width",
    "device-height",
    "min-device-height",
    "max-device-height",
    "aspect-ratio",
    "min-aspect-ratio",
    "max-aspect-ratio",
    "device-aspect-ratio",
    "min-device-aspect-ratio",
    "max-device-aspect-ratio",
    "color",
    "min-color",
    "max-color",
    "color-index",
    "min-color-index",
    "max-color-index",
    "monochrome",
    "min-monochrome",
    "max-monochrome",
    "-webkit-min-device-pixel-ratio",
    "resolution",
    "min-resolution",
    "max-resolution",
    "scan",
    "grid",
];

struct MediaQuery {
    media_type: String,
    negated: bool,
    features: Vec<String>,
}

/// A scoped media-query-list parser: `query (, query)*` where `query` is
/// `[not|only]? <ident> (and \( <feature-ident> (: ...)? \))*`. This is
/// not a general CSS3 Media Queries grammar (feature *values* -- the
/// `100px` in `(max-width: 100px)` -- are never parsed or evaluated,
/// only feature *names*, since [`IGNORED_MEDIA_FEATURES`] only ever
/// tests presence); real-world `@media`/`media=""` text in e-books is
/// overwhelmingly this shape. Returns `None` on anything it can't make
/// sense of, matching [`media_ok`]'s permissive fallback.
fn parse_media_query_list(raw: &str) -> Option<Vec<MediaQuery>> {
    let mut out = Vec::new();
    for part in raw.split(',') {
        out.push(parse_one_media_query(part.trim())?);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn parse_one_media_query(text: &str) -> Option<MediaQuery> {
    let lower = text.to_ascii_lowercase();
    let (negated, rest) = if let Some(r) = lower.strip_prefix("not ") {
        (true, &text[text.len() - r.len()..])
    } else if let Some(r) = lower.strip_prefix("only ") {
        (false, &text[text.len() - r.len()..])
    } else {
        (false, text)
    };
    let rest = rest.trim_start();
    let type_end = rest
        .find(|c: char| c.is_whitespace() || c == '(')
        .unwrap_or(rest.len());
    let media_type = rest[..type_end].trim().to_ascii_lowercase();
    if media_type.is_empty() {
        return None;
    }
    let mut features = Vec::new();
    let mut cursor = rest[type_end..].trim_start();
    loop {
        if let Some(r) = cursor.strip_prefix("and") {
            cursor = r.trim_start();
        }
        let Some(inner) = cursor.strip_prefix('(') else {
            break;
        };
        let close = inner.find(')')?;
        let feature_name = inner[..close]
            .split(':')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if !feature_name.is_empty() {
            features.push(feature_name);
        }
        cursor = inner[close + 1..].trim_start();
    }
    Some(MediaQuery {
        media_type,
        negated,
        features,
    })
}

fn query_ok(mq: &MediaQuery) -> bool {
    let mut matched = ALLOWED_MEDIA_TYPES.contains(&mq.media_type.as_str());
    for f in &mq.features {
        if IGNORED_MEDIA_FEATURES.contains(&f.as_str()) {
            matched = false;
        }
    }
    mq.negated ^ matched
}

/// Port of `media_ok`. `""` (Python's falsy `raw`) is always allowed.
/// See [`parse_media_query_list`]'s docs for the scoped grammar this
/// evaluates against, and its "unparseable -> allowed" fallback (Python:
/// `except Exception: pass; return True`).
pub fn media_ok(media_text: &str) -> bool {
    let raw = media_text.trim();
    if raw.is_empty() {
        return true;
    }
    if raw == "amzn-mobi" {
        return false;
    }
    match parse_media_query_list(raw) {
        None => true,
        Some(queries) => queries.iter().any(query_ok),
    }
}

/// Port of `media_allowed`: `None`/empty is always allowed.
pub fn media_allowed(media_text: Option<&str>) -> bool {
    match media_text {
        None => true,
        Some(t) if t.trim().is_empty() => true,
        Some(t) => media_ok(t),
    }
}

/// Port of `iterrules`: every rule in `sheet_name` (or `rules`, if
/// given), resolving `@import`/`@media` recursively and, if `rule_type`
/// is given, filtering to just that [`RuleType`]. Rules are returned as
/// owned clones (rather than borrowing from a cached parse the way
/// `get_xml`/`get_xhtml` do) since [`Container`] does not cache a
/// structured [`Stylesheet`] -- see [`Container::parsed_stylesheet`]'s
/// docs; this keeps `iterrules`' recursion free of any borrow conflict
/// with the `&mut Container` it needs for `href_to_name`/`has_name`/
/// re-parsing an imported sheet.
#[allow(clippy::too_many_arguments)]
pub fn iterrules(
    container: &mut Container,
    sheet_name: &str,
    rules: Option<&[Rule]>,
    rule_index_counter: &mut u64,
    rule_type: Option<RuleType>,
    importing: &mut std::collections::HashSet<String>,
) -> Result<Vec<(Rule, String, u64)>> {
    importing.insert(sheet_name.to_string());
    let owned_rules;
    let rules_slice: &[Rule] = match rules {
        Some(r) => r,
        None => {
            owned_rules = container.parsed_stylesheet(sheet_name)?.rules;
            &owned_rules
        }
    };
    let mut out = Vec::new();
    for rule in rules_slice {
        match rule {
            Rule::Import(imp) => {
                if media_allowed(imp.media_text.as_deref()) {
                    if let Some(name) = container.href_to_name(&imp.href, Some(sheet_name)) {
                        if container.has_name(&name) && !importing.contains(&name) {
                            let csheet = container.parsed_stylesheet(&name)?;
                            let sub = iterrules(
                                container,
                                &name,
                                Some(&csheet.rules),
                                rule_index_counter,
                                rule_type,
                                importing,
                            )?;
                            out.extend(sub);
                        }
                        // A recursive import (`name` already in
                        // `importing`) is silently ignored, matching
                        // the effect of Python's `container.log.error`
                        // path (this crate's `Container` has no
                        // logging facility to route the diagnostic
                        // through).
                    }
                }
            }
            Rule::Media(m) => {
                if media_allowed(Some(&m.media_text)) {
                    let sub = iterrules(
                        container,
                        sheet_name,
                        Some(&m.rules),
                        rule_index_counter,
                        rule_type,
                        importing,
                    )?;
                    out.extend(sub);
                }
            }
            other => {
                if rule_type.is_none() || Some(other.rule_type()) == rule_type {
                    let idx = *rule_index_counter;
                    *rule_index_counter += 1;
                    out.push((other.clone(), sheet_name.to_string(), idx));
                }
            }
        }
    }
    importing.remove(sheet_name);
    Ok(out)
}

/// Port of `iterdeclaration`: every declaration in `decl`, in source
/// order.
///
/// Python's version first runs each declaration through
/// `calibre.ebooks.oeb.normalize_css.normalizers` -- a table that
/// expands shorthand properties (`margin`, `border`, `font`,
/// `list-style`, ...) into their longhand equivalents before the
/// cascade sees them. This crate only ports [`crate::oeb::normalize_css::normalize_edge`]
/// (issue #35's scope: `margin`/`padding`/`border-style`/
/// `border-width`/`border-color`), not the full `normalizers` dict
/// (`border`/`border-<edge>`/`font`/`list-style` shorthand expansion).
/// This function is therefore a **documented narrowing, not a
/// `todo!()`**: it yields every declaration exactly as written, which
/// is correct behavior for any property Python's `normalizers.get(name)`
/// would also have returned `None` for (the overwhelming majority), and
/// under-expands only those five specific shorthands relative to
/// Python. Real-world stylesheets set the properties `stats.py`/
/// `cascade.py`'s callers actually resolve (`font-family`, `font-size`,
/// `font-weight`, `font-style`, `font-stretch`, `display`,
/// `text-transform`, `font-variant`, ...) directly far more often than
/// via those shorthands.
pub fn iterdeclaration(decl: &crate::css::StyleDeclarationBlock) -> Vec<crate::css::Declaration> {
    decl.properties.clone()
}

/// Port of `normalize_style_declaration`: every declaration in `decl`
/// (via [`iterdeclaration`]), with `font-family` values re-serialized
/// through [`parse_font_family`]/[`serialize_font_family`] (matching
/// Python's `cssutils`-workaround comment: normalizing whitespace/
/// quoting around commas in a `font-family` list).
pub fn normalize_style_declaration(
    decl: &crate::css::StyleDeclarationBlock,
    sheet_name: &str,
) -> HashMap<String, PropertyValue> {
    let mut ans = HashMap::new();
    for prop in iterdeclaration(decl) {
        let value = if prop.name.eq_ignore_ascii_case("font-family") {
            serialize_font_family(&parse_font_family(&prop.value))
        } else {
            prop.value
        };
        ans.insert(
            prop.name,
            PropertyValue::new(value, Some(sheet_name.to_string()), prop.important),
        );
    }
    ans
}

/// The two maps [`resolve_styles`] builds, ready to drive
/// [`resolve_property`]/[`resolve_pseudo_property`] directly -- the Rust
/// shape of Python's `partial(resolve_property, style_map)`/
/// `partial(resolve_pseudo_property, style_map, pseudo_style_map)`
/// closures (a bound closure over owned data doesn't need inventing here
/// since both functions already take `style_map`/`pseudo_style_map` as
/// explicit parameters).
pub struct ResolvedStyles {
    pub style_map: HashMap<NodeId, HashMap<String, PropertyValue>>,
    pub pseudo_style_map: HashMap<NodeId, HashMap<String, HashMap<String, PropertyValue>>>,
}

/// A callback invoked once per top-level sheet [`resolve_styles`]
/// processes (port of Python's `sheet_callback(sheet, sheet_name)`).
/// `container` is passed explicitly rather than captured so the
/// callback can itself call container methods -- e.g. `iterrules` for
/// `@font-face` rules, as `stats.rs`'s `collect_font_face_rules` does --
/// without conflicting with `resolve_styles`' own `&mut Container`
/// borrow.
pub type SheetCallback<'a> = dyn FnMut(&mut Container, &Stylesheet, &str) -> Result<()> + 'a;

/// Port of `resolve_styles`: resolves the full cascade for spine item
/// `name` -- the user-agent stylesheet, every linked/embedded
/// stylesheet and `<style>` tag, and every `style=""` attribute -- into
/// per-element declaration maps.
pub fn resolve_styles(
    container: &mut Container,
    name: &str,
    mut sheet_callback: Option<&mut SheetCallback<'_>>,
) -> Result<ResolvedStyles> {
    container.ensure_parsed(name)?;

    // Phase 1: pull everything needed out of the DOM into owned data,
    // so the borrow of `container.get_xhtml(name)` ends before any
    // `&mut Container` call below (href_to_name/parsed_stylesheet/...).
    struct StyleTag {
        text: String,
    }
    struct LinkTag {
        href: String,
    }
    let (style_tags, link_tags, style_attrs): (Vec<StyleTag>, Vec<LinkTag>, Vec<(NodeId, String)>) = {
        let dom = container.get_xhtml(name)?;
        let mut style_tags = Vec::new();
        let mut link_tags = Vec::new();
        let mut style_attrs = Vec::new();
        for id in dom.preorder_elements(dom.root) {
            match dom.tag(id) {
                Some("style") => {
                    let ty = dom
                        .node(id)
                        .attrs
                        .get("type")
                        .map(|s| s.as_str())
                        .unwrap_or("text/css");
                    if ty.eq_ignore_ascii_case("text/css") {
                        let text = dom.text_content(id);
                        if !text.trim().is_empty() {
                            style_tags.push(StyleTag { text });
                        }
                    }
                }
                Some("link") => {
                    let attrs = &dom.node(id).attrs;
                    let ty = attrs.get("type").map(|s| s.as_str()).unwrap_or("text/css");
                    let rel = attrs.get("rel").map(|s| s.as_str()).unwrap_or("stylesheet");
                    let media = attrs.get("media").map(|s| s.as_str()).unwrap_or("");
                    if let Some(href) = attrs.get("href") {
                        if crate::oeb::constants::OEB_STYLES
                            .iter()
                            .any(|m| m.eq_ignore_ascii_case(ty))
                            && rel.eq_ignore_ascii_case("stylesheet")
                            && media_ok(media)
                        {
                            link_tags.push(LinkTag { href: href.clone() });
                        }
                    }
                }
                _ => {}
            }
            if let Some(style) = dom.node(id).attrs.get("style") {
                if !style.trim().is_empty() {
                    style_attrs.push((id, style.clone()));
                }
            }
        }
        (style_tags, link_tags, style_attrs)
    };

    // Phase 2: process every top-level sheet (user-agent + <style> tags
    // + <link>ed sheets), collecting every STYLE_RULE (recursively
    // through @import/@media) with a shared, monotonic rule index --
    // `&mut Container` is free to use here since no Dom borrow is held.
    let mut rule_index_counter: u64 = 0;
    let mut collected: Vec<(crate::css::StyleRule, String, u64)> = Vec::new();
    let mut process_sheet = |container: &mut Container,
                             sheet: Stylesheet,
                             sheet_name: String,
                             rule_index_counter: &mut u64,
                             collected: &mut Vec<(crate::css::StyleRule, String, u64)>|
     -> Result<()> {
        if let Some(cb) = sheet_callback.as_mut() {
            cb(container, &sheet, &sheet_name)?;
        }
        let mut importing = std::collections::HashSet::new();
        let rules = iterrules(
            container,
            &sheet_name,
            Some(&sheet.rules),
            rule_index_counter,
            Some(RuleType::Style),
            &mut importing,
        )?;
        for (rule, rn, idx) in rules {
            if let Rule::Style(sr) = rule {
                collected.push((sr, rn, idx));
            }
        }
        Ok(())
    };

    process_sheet(
        container,
        html_css_stylesheet().clone(),
        "user-agent.css".to_string(),
        &mut rule_index_counter,
        &mut collected,
    )?;
    for style_tag in &style_tags {
        let sheet = Stylesheet::parse(&style_tag.text);
        process_sheet(
            container,
            sheet,
            name.to_string(),
            &mut rule_index_counter,
            &mut collected,
        )?;
    }
    for link in &link_tags {
        let Some(sheet_name) = container.href_to_name(&link.href, Some(name)) else {
            continue;
        };
        if !container.has_name(&sheet_name) {
            continue;
        }
        let sheet = container.parsed_stylesheet(&sheet_name)?;
        process_sheet(
            container,
            sheet,
            sheet_name,
            &mut rule_index_counter,
            &mut collected,
        )?;
    }

    // Phase 3: match every collected rule's selectors against the DOM
    // and bucket the resulting per-element declarations.
    let mut style_map: HashMap<NodeId, Vec<StyleDeclaration>> = HashMap::new();
    let mut pseudo_map: HashMap<NodeId, Vec<StyleDeclaration>> = HashMap::new();
    {
        let dom = container.get_xhtml(name)?;
        let select = Select::for_dom(dom);
        for (rule, sheet_name, rule_index) in &collected {
            let style = normalize_style_declaration(&rule.style, sheet_name);
            for selector in &rule.selectors.0 {
                let single = SelectorList(vec![selector.clone()]);
                let matches = select.matching(&single);
                let idx = specificity(*rule_index, selector.specificity, false);
                for elem in matches {
                    let decl = StyleDeclaration {
                        index: idx,
                        declaration: style.clone(),
                        pseudo_element: selector.pseudo_element.clone(),
                    };
                    if selector.pseudo_element.is_none() {
                        style_map.entry(elem.id).or_default().push(decl);
                    } else {
                        pseudo_map.entry(elem.id).or_default().push(decl);
                    }
                }
            }
        }
    }
    for (id, style_text) in &style_attrs {
        let decl_block = crate::css::parser::parse_declaration_list(style_text);
        let style = normalize_style_declaration(&decl_block, name);
        style_map.entry(*id).or_default().push(StyleDeclaration {
            index: Specificity {
                is_style: true,
                num_id: 0,
                num_class: 0,
                num_elem: 0,
                rule_index: 0,
            },
            declaration: style,
            pseudo_element: None,
        });
    }

    for decls in style_map.values_mut() {
        decls.sort_by_key(|d| std::cmp::Reverse(d.index));
    }
    for decls in pseudo_map.values_mut() {
        decls.sort_by_key(|d| std::cmp::Reverse(d.index));
    }

    let style_map = style_map
        .into_iter()
        .map(|(k, v)| (k, resolve_declarations(&v)))
        .collect();
    let pseudo_style_map = pseudo_map
        .into_iter()
        .map(|(k, v)| {
            let resolved = resolve_pseudo_declarations(&v);
            let by_name: HashMap<String, HashMap<String, PropertyValue>> = resolved
                .into_iter()
                .filter_map(|(k, v)| k.map(|k| (k, v)))
                .collect();
            (k, by_name)
        })
        .collect();

    Ok(ResolvedStyles {
        style_map,
        pseudo_style_map,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn pv(text: &str, important: bool) -> PropertyValue {
        PropertyValue::new(text, None, important)
    }

    fn make_container(files: &[(&str, &str, &str)]) -> (tempfile::TempDir, Container) {
        let dir = tempfile::tempdir().unwrap();
        let opf_path = dir.path().join("content.opf");
        let mut manifest_items = String::new();
        let mut spine_items = String::new();
        for (name, mt, content) in files {
            fs::write(dir.path().join(name), content).unwrap();
            manifest_items.push_str(&format!(
                r#"<item id="{name}" href="{name}" media-type="{mt}"/>"#
            ));
            if mt.contains("html") {
                spine_items.push_str(&format!(r#"<itemref idref="{name}"/>"#));
            }
        }
        let opf = format!(
            r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>T</dc:title><dc:identifier id="bookid">x</dc:identifier></metadata>
  <manifest>{manifest_items}</manifest>
  <spine>{spine_items}</spine>
</package>"#
        );
        fs::write(&opf_path, opf).unwrap();
        let container = Container::open(dir.path(), &opf_path).unwrap();
        (dir, container)
    }

    #[test]
    fn html_css_stylesheet_parses_the_bundled_user_agent_sheet_and_is_cached() {
        let sheet = html_css_stylesheet();
        // A handful of rules real callers depend on: block-level tags,
        // hidden elements, and the multi-selector `img, object, svg|svg`
        // rule that exercises the "drop the unsupported part, keep the
        // rest" selector-list behavior (see `css::selector`'s docs).
        assert!(sheet
            .style_rules()
            .any(|r| r.selector_text == "div, map, dt, isindex, form"));
        let img_rule = sheet
            .style_rules()
            .find(|r| r.selector_text.contains("img"))
            .expect("the img/object/svg|svg rule");
        assert_eq!(
            img_rule.selectors.0.len(),
            2,
            "svg|svg should be dropped, img/object kept"
        );
        assert!(
            std::ptr::eq(html_css_stylesheet(), html_css_stylesheet()),
            "must be cached, not reparsed"
        );
    }

    #[test]
    fn media_ok_allows_screen_and_empty_and_rejects_amzn_mobi() {
        assert!(media_ok(""));
        assert!(media_ok("screen"));
        assert!(media_ok("screen, print"));
        assert!(!media_ok("amzn-mobi"));
        assert!(media_allowed(None));
    }

    #[test]
    fn media_ok_rejects_a_pure_device_feature_query() {
        // `print` is not in ALLOWED_MEDIA_TYPES, so this query never
        // matches under any type; `min-width` is a device feature that
        // always fails regardless of value.
        assert!(!media_ok("print and (min-width: 400px)"));
        // `screen` alone (no feature) is allowed.
        assert!(media_ok("screen and (min-width: 400px), screen"));
    }

    #[test]
    fn resolve_styles_resolves_a_linked_sheet_inline_style_and_inheritance() {
        let (_dir, mut container) = make_container(&[
            (
                "index.html",
                "application/xhtml+xml",
                "<html><head><link rel=\"stylesheet\" href=\"style.css\"/></head><body><div class=\"a\">\
                 <p id=\"x\" style=\"font-weight: bold\">hi</p></div></body></html>",
            ),
            ("style.css", "text/css", "div.a { color: red; } p { color: blue; text-align: center }"),
        ]);

        let resolved = resolve_styles(&mut container, "index.html", None).unwrap();
        let dom = container.get_xhtml("index.html").unwrap();
        let p = dom.find_first_tag_global("p").unwrap();

        // The inline style="" attribute (specificity trumps everything)
        // wins for font-weight.
        let fw = resolve_property(&resolved.style_map, dom, p, "font-weight").unwrap();
        assert_eq!(fw.css_text, "bold");
        // `p { color: blue }` has higher specificity (element selector on
        // `p` directly) than the inherited `div.a { color: red }` --
        // both are real candidates, but `color` was set directly on `p`
        // so no inheritance walk is even needed.
        let color = resolve_property(&resolved.style_map, dom, p, "color").unwrap();
        assert_eq!(color.css_text, "blue");
        let ta = resolve_property(&resolved.style_map, dom, p, "text-align").unwrap();
        assert_eq!(ta.css_text, "center");
    }

    #[test]
    fn resolve_styles_resolves_pseudo_element_declarations() {
        let (_dir, mut container) = make_container(&[(
            "index.html",
            "application/xhtml+xml",
            "<html><head><style>.fl::first-line { font-family: X }</style></head>\
             <body><p class=\"fl\">abc</p></body></html>",
        )]);
        let resolved = resolve_styles(&mut container, "index.html", None).unwrap();
        let dom = container.get_xhtml("index.html").unwrap();
        let p = dom.find_first_tag_global("p").unwrap();
        let val = resolve_pseudo_property(
            &resolved.style_map,
            &resolved.pseudo_style_map,
            dom,
            p,
            "first-line",
            "font-family",
            false,
            false,
            false,
        )
        .unwrap();
        assert_eq!(val.css_text, "X");
    }

    #[test]
    fn iterrules_resolves_import_and_media_and_assigns_monotonic_indices() {
        let (_dir, mut container) = make_container(&[
            (
                "index.html",
                "application/xhtml+xml",
                "<html><body/></html>",
            ),
            (
                "a.css",
                "text/css",
                "@import url(b.css); .a { color: red } @media screen { .c { color: green } }",
            ),
            ("b.css", "text/css", ".b { color: blue }"),
        ]);
        let mut counter = 0u64;
        let mut importing = std::collections::HashSet::new();
        let rules = iterrules(
            &mut container,
            "a.css",
            None,
            &mut counter,
            Some(RuleType::Style),
            &mut importing,
        )
        .unwrap();
        let selectors: Vec<String> = rules
            .iter()
            .filter_map(|(r, _, _)| r.as_style().map(|s| s.selector_text.clone()))
            .collect();
        assert!(selectors.contains(&".b".to_string()), "{selectors:?}");
        assert!(selectors.contains(&".a".to_string()), "{selectors:?}");
        assert!(selectors.contains(&".c".to_string()), "{selectors:?}");
        // Indices are unique and monotonic across the whole recursive
        // walk (import + media).
        let mut indices: Vec<u64> = rules.iter().map(|(_, _, idx)| *idx).collect();
        indices.sort_unstable();
        indices.dedup();
        assert_eq!(indices.len(), rules.len());
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
