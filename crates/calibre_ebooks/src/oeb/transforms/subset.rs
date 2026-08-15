//! Port of `old_src/src/calibre/ebooks/oeb/transforms/subset.py`.
//!
//! Distinct from [`crate::oeb::polish::subset`] (issue #164's retrofit of
//! the *Polish Book* editor's font-subsetting tool, which operates on
//! `polish::Container`). This is the conversion-pipeline transform that
//! runs against the plain `OEBBook`/[`Dom`] world this module operates
//! in, and it is also where `embed_fonts.rs`'s `elem_style`/
//! `find_font_face_rules`/`get_font_properties` come from (both files
//! import them from `subset.py` in Python).
//!
//! # Font-family/weight/style/stretch extraction: real
//!
//! [`get_font_properties`]/[`find_font_face_rules`]/[`elem_style`]/
//! [`find_style_rules`] are pure value extraction and matching over an
//! already-parsed [`crate::css::Stylesheet`] (issue #164) plus
//! [`crate::oeb::fonts3::parse_font_family`] (issue #164's
//! `font-family` shorthand parser) -- no external capability needed, so
//! these are ported for real and are independently unit-tested.
//!
//! # The one gap: actual glyph subsetting
//!
//! [`SubsetFonts`] finds which characters each embedded font is actually
//! used for ([`SubsetFonts::find_font_usage`]/[`find_chars`]) for real --
//! that is pure DOM/style-map walking. Deciding a font is **unused** and
//! removing it from the manifest is also real (no external dependency).
//! Only the byte-level work for a font that *is* used --
//! `calibre.utils.fonts.subset.subset` (TrueType/OpenType `cmap`/`glyf`/
//! `loca` table rewriting) -- is out of scope: this is the same,
//! already-documented gap [`crate::oeb::polish::subset::subset_all_fonts`]
//! left open (issue #164). [`subset_font_bytes`] is the narrow `todo!()`
//! site for it, called only when a font actually has characters to keep
//! (an unused font never reaches it).

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::css::{Rule, StyleDeclarationBlock, Stylesheet};
use crate::mobi::dom::{Dom, NodeId, NodeKind};
use crate::oeb::book::OEBBook;
use crate::oeb::fonts3::parse_font_family;

/// Port of `font_properties` (the tuple of CSS properties this file
/// extracts from a rule).
pub const FONT_PROPERTIES: &[&str] = &[
    "font-family",
    "src",
    "font-weight",
    "font-stretch",
    "font-style",
    "text-transform",
];

/// Port of the per-element/per-rule font-property dict `get_font_properties`
/// builds (minus `src`, which only applies to `@font-face` rules and is
/// returned separately by [`get_font_properties`]). Each field is `None`
/// when the property was absent, invalid, or (with no `default`) equal to
/// `inherit`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ElemStyle {
    pub font_family: Option<Vec<String>>,
    pub font_weight: Option<String>,
    pub font_style: Option<String>,
    pub font_stretch: Option<String>,
    pub text_transform: Option<String>,
}

impl ElemStyle {
    /// `true` iff every field is `None` (Python's `if not props`).
    pub fn is_empty(&self) -> bool {
        self.font_family.is_none()
            && self.font_weight.is_none()
            && self.font_style.is_none()
            && self.font_stretch.is_none()
            && self.text_transform.is_none()
    }

    /// Overlay `other`'s `Some` fields onto `self` (Python's `dict.update`).
    fn update(&mut self, other: &ElemStyle) {
        if other.font_family.is_some() {
            self.font_family = other.font_family.clone();
        }
        if other.font_weight.is_some() {
            self.font_weight = other.font_weight.clone();
        }
        if other.font_style.is_some() {
            self.font_style = other.font_style.clone();
        }
        if other.font_stretch.is_some() {
            self.font_stretch = other.font_stretch.clone();
        }
        if other.text_transform.is_some() {
            self.text_transform = other.text_transform.clone();
        }
    }
}

const VALID_WEIGHTS: &[&str] = &[
    "100", "200", "300", "400", "500", "600", "700", "800", "900", "bolder", "lighter",
];
const VALID_STYLES: &[&str] = &["normal", "italic", "oblique"];
const VALID_STRETCHES: &[&str] = &[
    "normal",
    "ultra-condensed",
    "extra-condensed",
    "condensed",
    "semi-condensed",
    "semi-expanded",
    "expanded",
    "extra-expanded",
    "ultra-expanded",
];

/// Extracts the first `url(...)` (or bare, unquoted) token from a `src`
/// declaration's value, matching Python's `propertyValue[0].uri` (the
/// first value in a possibly comma-separated `src` list, e.g.
/// `src: url(a.otf), url(b.otf)`).
fn first_uri(value: &str) -> Option<String> {
    let first = value.split(',').next()?.trim();
    let inner = if let Some(rest) = first.strip_prefix("url(") {
        rest.strip_suffix(')').unwrap_or(rest)
    } else {
        first
    };
    let inner = inner.trim().trim_matches(|c| c == '"' || c == '\'');
    if inner.is_empty() {
        None
    } else {
        Some(inner.to_string())
    }
}

/// Port of `get_font_properties`: given a rule's already-parsed
/// declaration block, extracts normalized font properties. Returns
/// `(style, src)` -- `src` is only ever populated when the block actually
/// declares one (i.e. from a `@font-face` rule).
pub fn get_font_properties(
    style: &StyleDeclarationBlock,
    default: Option<&str>,
) -> (ElemStyle, Option<String>) {
    let mut out = ElemStyle::default();

    let ff_raw = style.get_property_value("font-family");
    if !ff_raw.is_empty() {
        let parsed = parse_font_family(ff_raw);
        if !parsed.is_empty() && !parsed[0].eq_ignore_ascii_case("inherit") {
            out.font_family = Some(parsed);
        }
    }

    let src = {
        let raw = style.get_property_value("src");
        if raw.is_empty() {
            None
        } else {
            first_uri(raw)
        }
    };

    let mut weight = nonempty(style.get_property_value("font-weight")).map(|s| s.to_lowercase());
    if weight.as_deref() == Some("inherit") {
        weight = default.map(str::to_string);
    }
    weight = match weight.as_deref() {
        Some("normal") => Some("400".to_string()),
        Some("bold") => Some("700".to_string()),
        _ => weight,
    };
    if !weight
        .as_deref()
        .map(|v| VALID_WEIGHTS.contains(&v))
        .unwrap_or(false)
    {
        weight = default.map(str::to_string);
    }
    out.font_weight = weight;

    let mut fstyle = nonempty(style.get_property_value("font-style")).map(|s| s.to_lowercase());
    if fstyle.as_deref() == Some("inherit") {
        fstyle = default.map(str::to_string);
    }
    if !fstyle
        .as_deref()
        .map(|v| VALID_STYLES.contains(&v))
        .unwrap_or(false)
    {
        fstyle = default.map(str::to_string);
    }
    out.font_style = fstyle;

    let mut fstretch = nonempty(style.get_property_value("font-stretch")).map(|s| s.to_lowercase());
    if fstretch.as_deref() == Some("inherit") {
        fstretch = default.map(str::to_string);
    }
    if !fstretch
        .as_deref()
        .map(|v| VALID_STRETCHES.contains(&v))
        .unwrap_or(false)
    {
        fstretch = default.map(str::to_string);
    }
    out.font_stretch = fstretch;

    let tt = nonempty(style.get_property_value("text-transform"));
    out.text_transform = tt
        .map(str::to_string)
        .or_else(|| default.map(str::to_string));

    (out, src)
}

fn nonempty(s: &str) -> Option<&str> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// A `@font-face` rule as extracted by [`find_font_face_rules`]: the CSS
/// specification side (family always present, weight/style/stretch
/// defaulted to `"normal"`), plus which manifest item the `src` resolved
/// to and the set of characters (as discovered by
/// [`SubsetFonts::find_font_usage`]) the book actually uses from it.
#[derive(Clone, Debug)]
pub struct FontFaceInfo {
    pub style: ElemStyle,
    /// `font-weight` as an integer (`bolder`/`lighter` normalized to 400,
    /// matching Python's `props['weight'] = int(props['font-weight'])`
    /// after that normalization).
    pub weight: i32,
    /// The manifest href the rule's `src` resolved to.
    pub item_href: String,
    pub chars: HashSet<char>,
}

/// Port of `find_font_face_rules`: every `@font-face` rule in `sheet`
/// (parsed from a stylesheet whose own href, for resolving `src`
/// relative URLs, is `sheet_href`) that has both a `font-family` and a
/// `src` resolving to a real manifest item.
pub fn find_font_face_rules(
    sheet: &Stylesheet,
    sheet_href: &str,
    oeb: &OEBBook,
) -> Vec<FontFaceInfo> {
    let mut out = Vec::new();
    for decl in sheet.font_face_rules() {
        let (mut style, src) = get_font_properties(decl, Some("normal"));
        let (Some(_), Some(src)) = (&style.font_family, &src) else {
            continue;
        };
        let path = super::filenames::abshref(sheet_href, src);
        let Some(item) = oeb.manifest.get_by_href(&path) else {
            continue;
        };
        let mut weight_str = style
            .font_weight
            .clone()
            .unwrap_or_else(|| "400".to_string());
        if weight_str == "bolder" || weight_str == "lighter" {
            weight_str = "400".to_string();
        }
        let weight: i32 = weight_str.parse().unwrap_or(400);
        style.font_weight = Some(weight_str);
        out.push(FontFaceInfo {
            style,
            weight,
            item_href: item.href.clone(),
            chars: HashSet::new(),
        });
    }
    out
}

/// Port of `find_style_rules`: for every `STYLE_RULE` in `sheet` with at
/// least one usable font property, folds it into `rules` keyed by each of
/// the rule's plain-class selectors (`.foo` -- not pseudo-selectors,
/// matching Python's `sel.partition(':')[0]`).
pub fn find_style_rules(sheet: &Stylesheet, rules: &mut HashMap<String, ElemStyle>) {
    for rule in sheet.style_rules() {
        let (props, _src) = get_font_properties(&rule.style, None);
        if props.is_empty() {
            continue;
        }
        for selector in &rule.selectors.0 {
            let sel = selector.text.trim();
            if let Some(rest) = sel.strip_prefix('.') {
                let class = rest.split(':').next().unwrap_or(rest);
                if class.is_empty() {
                    continue;
                }
                rules.entry(class.to_string()).or_default().update(&props);
            }
        }
    }
}

const STRETCH_ORDER: &[&str] = &[
    "ultra-condensed",
    "extra-condensed",
    "condensed",
    "semi-condensed",
    "normal",
    "semi-expanded",
    "expanded",
    "extra-expanded",
    "ultra-expanded",
];

fn stretch_index(val: &str) -> usize {
    STRETCH_ORDER.iter().position(|&s| s == val).unwrap_or(4)
}

/// Port of `elem_style`: the effective style for an element carrying
/// `cls` (a possibly space-separated `class` attribute value), given the
/// class -> properties table from [`find_style_rules`] and the style
/// inherited from the element's parent.
pub fn elem_style(
    style_rules: &HashMap<String, ElemStyle>,
    cls: &str,
    inherited_style: &ElemStyle,
) -> ElemStyle {
    let mut style = inherited_style.clone();
    for c in cls.split_whitespace() {
        if let Some(over) = style_rules.get(c) {
            style.update(over);
        }
    }
    let pwt = inherited_style
        .font_weight
        .clone()
        .unwrap_or_else(|| "400".to_string());
    match style.font_weight.as_deref() {
        Some("bolder") => {
            let mapped = match pwt.as_str() {
                "100" | "200" | "300" => "400",
                "400" | "500" => "700",
                _ => "900",
            };
            style.font_weight = Some(mapped.to_string());
        }
        Some("lighter") => {
            let mapped = match pwt.as_str() {
                "600" | "700" => "400",
                "800" | "900" => "700",
                _ => "100",
            };
            style.font_weight = Some(mapped.to_string());
        }
        _ => {}
    }
    style
}

/// Port of `SubsetFonts.used_font`: the index into `embedded_fonts` of
/// the font best matching `style`, or `None` if no family matches at all.
pub fn used_font(style: &ElemStyle, embedded_fonts: &[FontFaceInfo]) -> Option<usize> {
    let lnames: HashSet<String> = style
        .font_family
        .as_ref()?
        .iter()
        .map(|f| f.to_lowercase())
        .collect();
    if lnames.is_empty() {
        return None;
    }
    let mut matching: Vec<usize> = (0..embedded_fonts.len())
        .filter(|&i| {
            let flnames: HashSet<String> = embedded_fonts[i]
                .style
                .font_family
                .as_ref()
                .map(|v| v.iter().map(|f| f.to_lowercase()).collect())
                .unwrap_or_default();
            lnames.intersection(&flnames).next().is_some()
        })
        .collect();
    if matching.is_empty() {
        return None;
    }

    let want_stretch = stretch_index(style.font_stretch.as_deref().unwrap_or("normal"));
    let widths: HashMap<usize, usize> = matching
        .iter()
        .map(|&i| {
            (
                i,
                stretch_index(
                    embedded_fonts[i]
                        .style
                        .font_stretch
                        .as_deref()
                        .unwrap_or("normal"),
                ),
            )
        })
        .collect();
    let min_dist = matching
        .iter()
        .map(|&i| (want_stretch as i64 - widths[&i] as i64).abs())
        .min()
        .unwrap_or(0);
    let nearest: Vec<usize> = matching
        .iter()
        .copied()
        .filter(|&i| (want_stretch as i64 - widths[&i] as i64).abs() == min_dist)
        .collect();
    let lmatches: Vec<usize> = if want_stretch <= 4 {
        nearest
            .iter()
            .copied()
            .filter(|&i| widths[&i] <= want_stretch)
            .collect()
    } else {
        nearest
            .iter()
            .copied()
            .filter(|&i| widths[&i] >= want_stretch)
            .collect()
    };
    matching = if !lmatches.is_empty() {
        lmatches
    } else {
        nearest
    };

    let fs = style.font_style.as_deref().unwrap_or("normal");
    let order: &[&str] = match fs {
        "oblique" => &["oblique", "italic", "normal"],
        "normal" => &["normal", "oblique", "italic"],
        _ => &["italic", "oblique", "normal"],
    };
    for &q in order {
        let m: Vec<usize> = matching
            .iter()
            .copied()
            .filter(|&i| {
                embedded_fonts[i]
                    .style
                    .font_style
                    .as_deref()
                    .unwrap_or("normal")
                    == q
            })
            .collect();
        if !m.is_empty() {
            matching = m;
            break;
        }
    }

    let fw: i32 = style
        .font_weight
        .as_deref()
        .unwrap_or("400")
        .parse()
        .unwrap_or(400);
    let candidates: Vec<i32> = if fw == 400 {
        vec![400, 500, 300, 200, 100, 600, 700, 800, 900]
    } else if fw == 500 {
        vec![500, 400, 300, 200, 100, 600, 700, 800, 900]
    } else if fw < 400 {
        let mut v = vec![fw];
        let mut x = fw - 100;
        while x > -100 {
            v.push(x);
            x -= 100;
        }
        v
    } else {
        let mut v = vec![fw];
        let mut x = fw + 100;
        while x < 1000 {
            v.push(x);
            x += 100;
        }
        let mut x = fw - 100;
        while x > -100 {
            v.push(x);
            x -= 100;
        }
        v
    };
    for wt in candidates {
        if let Some(&i) = matching.iter().find(|&&i| embedded_fonts[i].weight == wt) {
            return Some(i);
        }
    }
    None
}

/// Port of `SubsetFonts.find_chars`: every character `elem`'s own direct
/// text (and, in this arena's sibling-based `.tail` representation, every
/// child's trailing text) contributes, after applying `style`'s
/// `text-transform`.
pub fn find_chars(dom: &Dom, elem: NodeId, style: &ElemStyle) -> HashSet<char> {
    let mut ans = HashSet::new();
    let transform = |s: &str| -> String {
        match style.text_transform.as_deref() {
            Some("uppercase") | Some("capitalize") => calibre_utils::icu::upper(s),
            Some("lowercase") => calibre_utils::icu::lower(s),
            _ => s.to_string(),
        }
    };
    for child in dom.children(elem) {
        if let NodeKind::Text(t) = &dom.node(child).kind {
            ans.extend(transform(t).chars());
        }
    }
    ans
}

/// Port of `SubsetFonts` (minus the actual byte-level subsetting -- see
/// the module docs).
pub struct SubsetFonts {
    pub embedded_fonts: Vec<FontFaceInfo>,
    pub style_rules: HashMap<String, ElemStyle>,
}

impl Default for SubsetFonts {
    fn default() -> Self {
        Self::new()
    }
}

impl SubsetFonts {
    pub fn new() -> Self {
        SubsetFonts {
            embedded_fonts: Vec::new(),
            style_rules: HashMap::new(),
        }
    }

    /// Port of `SubsetFonts.find_embedded_fonts`.
    pub fn find_embedded_fonts(&mut self, oeb: &OEBBook) {
        self.embedded_fonts.clear();
        let sheets: Vec<(String, Vec<u8>)> = oeb
            .manifest
            .iter()
            .filter(|i| crate::oeb::constants::OEB_STYLES.contains(&i.media_type.as_str()))
            .filter_map(|i| {
                oeb.container
                    .read(&i.href)
                    .ok()
                    .map(|d| (i.href.clone(), d))
            })
            .collect();
        for (href, data) in sheets {
            let text = String::from_utf8_lossy(&data);
            let sheet = Stylesheet::parse(&text);
            self.embedded_fonts
                .extend(find_font_face_rules(&sheet, &href, oeb));
        }
    }

    /// Port of `SubsetFonts.find_style_rules`.
    pub fn find_style_rules(&mut self, oeb: &OEBBook) {
        self.style_rules.clear();
        let sheets: Vec<Vec<u8>> = oeb
            .manifest
            .iter()
            .filter(|i| crate::oeb::constants::OEB_STYLES.contains(&i.media_type.as_str()))
            .filter_map(|i| oeb.container.read(&i.href).ok())
            .collect();
        for data in sheets {
            let text = String::from_utf8_lossy(&data);
            let sheet = Stylesheet::parse(&text);
            find_style_rules(&sheet, &mut self.style_rules);
        }
    }

    fn find_usage_in(&mut self, dom: &Dom, elem: NodeId, inherited_style: &ElemStyle) {
        let cls = dom
            .node(elem)
            .attrs
            .get("class")
            .cloned()
            .unwrap_or_default();
        let style = elem_style(&self.style_rules, &cls, inherited_style);
        for child in dom.children(elem) {
            if matches!(dom.node(child).kind, NodeKind::Element(_)) {
                self.find_usage_in(dom, child, &style);
            }
        }
        if let Some(idx) = used_font(&style, &self.embedded_fonts) {
            let chars = find_chars(dom, elem, &style);
            if !chars.is_empty() {
                self.embedded_fonts[idx].chars.extend(chars);
            }
        }
    }

    /// Port of `SubsetFonts.find_font_usage`.
    pub fn find_font_usage(&mut self, oeb: &OEBBook) {
        let base = ElemStyle {
            font_family: Some(vec!["serif".to_string()]),
            font_weight: Some("400".to_string()),
            font_style: Some("normal".to_string()),
            font_stretch: Some("normal".to_string()),
            text_transform: None,
        };
        let spine_hrefs: Vec<String> = oeb
            .spine
            .iter()
            .filter_map(|s| oeb.manifest.get_by_id(&s.idref).map(|i| i.href.clone()))
            .collect();
        for href in spine_hrefs {
            let Ok(raw) = oeb.container.read(&href) else {
                continue;
            };
            let html = String::from_utf8_lossy(&raw);
            let dom = Dom::parse(&html);
            for body in dom.find_all_tag_global("body") {
                self.find_usage_in(&dom, body, &base);
            }
        }
    }

    /// Port of `SubsetFonts.__call__`. `report` mirrors Python's `log`
    /// callback. Returns without error (and without touching anything)
    /// if there are no embedded fonts at all -- matching Python's early
    /// return -- and never calls [`subset_font_bytes`] for a font with no
    /// recorded usage (that font is removed outright instead, which
    /// needs no external subsetter).
    pub fn call(&mut self, oeb: &mut OEBBook, report: &mut dyn FnMut(&str)) -> Result<()> {
        self.find_embedded_fonts(oeb);
        if self.embedded_fonts.is_empty() {
            report("No embedded fonts found");
            return Ok(());
        }
        self.find_style_rules(oeb);
        self.find_font_usage(oeb);

        // Python merges by `item.href` (several `@font-face` rules can
        // point at the same embedded file); do the same.
        let mut merged: HashMap<String, HashSet<char>> = HashMap::new();
        for f in &self.embedded_fonts {
            merged
                .entry(f.item_href.clone())
                .or_default()
                .extend(&f.chars);
        }

        for (href, chars) in merged {
            if chars.is_empty() {
                report(&format!("The font {href} is unused. Removing it."));
                self.remove_font(oeb, &href)?;
                continue;
            }
            // A font that is actually used needs real byte-level
            // subsetting -- see the module docs.
            subset_font_bytes(oeb, &href, &chars, report)?;
        }
        Ok(())
    }

    fn remove_font(&self, oeb: &mut OEBBook, href: &str) -> Result<()> {
        if let Some(item) = oeb.manifest.get_by_href(href) {
            let id = item.id.clone();
            oeb.manifest.remove(&id);
        }
        // Remove the @font-face rule(s) pointing at this file from every
        // stylesheet -- reuses the CSS-parsing gap already closed by
        // `oeb::polish::subset::remove_font_face_rules`'s logic, adapted
        // to this module's raw-container-bytes world (no `polish::Container`
        // cache to keep in sync).
        let sheets: Vec<(String, Vec<u8>)> = oeb
            .manifest
            .iter()
            .filter(|i| crate::oeb::constants::OEB_STYLES.contains(&i.media_type.as_str()))
            .filter_map(|i| {
                oeb.container
                    .read(&i.href)
                    .ok()
                    .map(|d| (i.href.clone(), d))
            })
            .collect();
        for (sheet_href, data) in sheets {
            let text = String::from_utf8_lossy(&data);
            let mut sheet = Stylesheet::parse(&text);
            let mut changed = false;
            sheet.rules.retain(|rule| {
                let Rule::FontFace(decl) = rule else {
                    return true;
                };
                let src = decl.get_property_value("src");
                let Some(uri) = first_uri(src) else {
                    return true;
                };
                let path = super::filenames::abshref(&sheet_href, &uri);
                if path == href {
                    changed = true;
                    false
                } else {
                    true
                }
            });
            if changed {
                let _ = oeb
                    .container
                    .write(&sheet_href, sheet.to_css_text().as_bytes());
            }
        }
        Ok(())
    }
}

/// The one real gap: byte-level TrueType/OpenType glyph subsetting for
/// the font at `href`, keeping only `chars`. Needs
/// `calibre.utils.fonts.subset.subset` -- the same, already-documented
/// gap left open by [`crate::oeb::polish::subset::subset_all_fonts`]
/// (issue #164): a substantial binary table editor (`cmap`/`glyf`/`loca`
/// rewriting) this crate has no equivalent for.
pub fn subset_font_bytes(
    _oeb: &mut OEBBook,
    _href: &str,
    _chars: &HashSet<char>,
    _report: &mut dyn FnMut(&str),
) -> Result<()> {
    todo!(
        "placeholder: needs a real TrueType/OpenType font subsetter \
         (calibre.utils.fonts.subset.subset: cmap/glyf/loca table \
         rewriting), which this crate has no equivalent for -- the same \
         gap left open by oeb::polish::subset::subset_all_fonts (issue #164)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::parser::parse_declaration_list;
    use crate::oeb::transforms::test_support::Builder;

    fn decl(css: &str) -> StyleDeclarationBlock {
        parse_declaration_list(css)
    }

    #[test]
    fn get_font_properties_normalizes_weight_style_stretch() {
        let d = decl("font-weight: bold; font-style: ITALIC; font-stretch: condensed");
        let (props, src) = get_font_properties(&d, None);
        assert_eq!(props.font_weight.as_deref(), Some("700"));
        assert_eq!(props.font_style.as_deref(), Some("italic"));
        assert_eq!(props.font_stretch.as_deref(), Some("condensed"));
        assert_eq!(src, None);
    }

    #[test]
    fn get_font_properties_falls_back_to_default_for_invalid_values() {
        let d = decl("font-weight: not-a-weight");
        let (props, _) = get_font_properties(&d, Some("normal"));
        assert_eq!(props.font_weight.as_deref(), Some("normal"));
    }

    #[test]
    fn get_font_properties_parses_font_family_and_drops_inherit() {
        let d = decl("font-family: \"My Font\", serif");
        let (props, _) = get_font_properties(&d, None);
        assert_eq!(
            props.font_family,
            Some(vec!["My Font".to_string(), "serif".to_string()])
        );
        let d2 = decl("font-family: inherit");
        let (props2, _) = get_font_properties(&d2, None);
        assert_eq!(props2.font_family, None);
    }

    #[test]
    fn find_font_face_rules_resolves_src_against_manifest() {
        let oeb = Builder::new()
            .part("fonts/a.otf", "font/otf", b"fontdata", false)
            .build();
        let sheet = Stylesheet::parse(
            "@font-face { font-family: 'My Font'; src: url(fonts/a.otf); font-weight: bold }",
        );
        let rules = find_font_face_rules(&sheet, "style.css", &oeb);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].item_href, "fonts/a.otf");
        assert_eq!(rules[0].weight, 700);
        assert_eq!(
            rules[0].style.font_family,
            Some(vec!["My Font".to_string()])
        );
    }

    #[test]
    fn find_font_face_rules_skips_rules_with_unresolvable_src() {
        let oeb = Builder::new().build();
        let sheet = Stylesheet::parse("@font-face { font-family: 'X'; src: url(missing.otf) }");
        let rules = find_font_face_rules(&sheet, "style.css", &oeb);
        assert!(rules.is_empty());
    }

    #[test]
    fn elem_style_applies_class_overrides_and_bolder_lighter() {
        let mut style_rules = HashMap::new();
        style_rules.insert(
            "big".to_string(),
            ElemStyle {
                font_weight: Some("bolder".to_string()),
                ..ElemStyle::default()
            },
        );
        let inherited = ElemStyle {
            font_family: Some(vec!["serif".to_string()]),
            font_weight: Some("400".to_string()),
            font_style: Some("normal".to_string()),
            font_stretch: Some("normal".to_string()),
            text_transform: None,
        };
        let style = elem_style(&style_rules, "big", &inherited);
        assert_eq!(style.font_weight.as_deref(), Some("700"));
        assert_eq!(style.font_family, inherited.font_family);
    }

    #[test]
    fn used_font_matches_on_family_then_narrows_by_weight() {
        let f1 = FontFaceInfo {
            style: ElemStyle {
                font_family: Some(vec!["My Font".to_string()]),
                font_weight: Some("400".to_string()),
                font_style: Some("normal".to_string()),
                font_stretch: Some("normal".to_string()),
                text_transform: None,
            },
            weight: 400,
            item_href: "fonts/regular.otf".to_string(),
            chars: HashSet::new(),
        };
        let f2 = FontFaceInfo {
            style: ElemStyle {
                font_family: Some(vec!["My Font".to_string()]),
                font_weight: Some("700".to_string()),
                font_style: Some("normal".to_string()),
                font_stretch: Some("normal".to_string()),
                text_transform: None,
            },
            weight: 700,
            item_href: "fonts/bold.otf".to_string(),
            chars: HashSet::new(),
        };
        let fonts = vec![f1, f2];
        let style = ElemStyle {
            font_family: Some(vec!["My Font".to_string()]),
            font_weight: Some("700".to_string()),
            font_style: Some("normal".to_string()),
            font_stretch: Some("normal".to_string()),
            text_transform: None,
        };
        let idx = used_font(&style, &fonts).unwrap();
        assert_eq!(fonts[idx].item_href, "fonts/bold.otf");
    }

    #[test]
    fn used_font_returns_none_when_no_family_matches() {
        let f1 = FontFaceInfo {
            style: ElemStyle {
                font_family: Some(vec!["Other".to_string()]),
                font_weight: Some("400".to_string()),
                font_style: Some("normal".to_string()),
                font_stretch: Some("normal".to_string()),
                text_transform: None,
            },
            weight: 400,
            item_href: "fonts/o.otf".to_string(),
            chars: HashSet::new(),
        };
        let style = ElemStyle {
            font_family: Some(vec!["My Font".to_string()]),
            ..ElemStyle::default()
        };
        assert!(used_font(&style, &[f1]).is_none());
    }

    #[test]
    fn find_chars_collects_transformed_text() {
        let dom = Dom::parse("<p>abc <em>DEF</em></p>");
        let p = dom.find_first_tag_global("p").unwrap();
        let style = ElemStyle {
            text_transform: Some("uppercase".to_string()),
            ..ElemStyle::default()
        };
        let chars = find_chars(&dom, p, &style);
        assert!(chars.contains(&'A'));
        assert!(chars.contains(&' '));
    }

    #[test]
    fn subset_fonts_removes_a_font_with_no_recorded_usage() {
        let mut oeb = Builder::new()
            .part(
                "style.css",
                "text/css",
                b"@font-face { font-family: 'Unused'; src: url(fonts/u.otf) }",
                false,
            )
            .part("fonts/u.otf", "font/otf", b"fontdata", false)
            .page("a.html", "<p>hi</p>")
            .build();
        let mut subsetter = SubsetFonts::new();
        let mut log = Vec::new();
        subsetter
            .call(&mut oeb, &mut |m| log.push(m.to_string()))
            .unwrap();
        assert!(oeb.manifest.get_by_href("fonts/u.otf").is_none());
        assert!(log.iter().any(|m| m.contains("unused")));
    }

    #[test]
    fn subset_fonts_is_a_no_op_when_nothing_is_embedded() {
        let mut oeb = Builder::new().page("a.html", "<p>hi</p>").build();
        let mut subsetter = SubsetFonts::new();
        let mut log = Vec::new();
        subsetter
            .call(&mut oeb, &mut |m| log.push(m.to_string()))
            .unwrap();
        assert!(log.iter().any(|m| m.contains("No embedded fonts")));
    }
}
