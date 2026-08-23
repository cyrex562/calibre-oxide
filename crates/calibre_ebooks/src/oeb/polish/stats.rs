//! Port of `old_src/src/calibre/ebooks/oeb/polish/stats.py`:
//! `StatsCollector`, per-language character/font-usage statistics
//! collection over a whole book. Depends on `cascade.py`'s
//! `iterdeclaration`/`iterrules`/`resolve_styles` -- real end to end as
//! of issue #164 (see `cascade.rs`'s module docs) -- so this file ports
//! for real too, plus `tinycss.fonts3.parse_font_family` (ported
//! directly, per issue #164's own scoping note, as plain string parsing
//! rather than routed through [`crate::css`] -- see
//! [`crate::oeb::fonts3`]).
//!
//! # Divergences from Python
//!
//! - **No ICU (`calibre.utils.icu`).** `icu_lower`/`icu_upper` become
//!   Rust's built-in (Unicode-aware, but locale-independent)
//!   `str::to_lowercase`/`str::to_uppercase`; `ord_string`/`safe_chr`
//!   (which round-trip text through Unicode code points, working around
//!   narrow-Python-build limitations that don't apply here) are simply
//!   not needed -- this port keeps character sets as `HashSet<char>`
//!   throughout, `char` already being a valid Unicode scalar value.
//! - **Font-family/style pattern matching still uses the `regex` crate**
//!   (already a dependency), whose Unicode property classes (`\p{L}`,
//!   `\p{N}`, `\p{P}`) cover `first_letter_pat`/`capitalize_pat`
//!   directly; no behavior change from Python's `regex` module use here.
//! - **`get_css_text`/`get_element_text`** work against
//!   [`crate::oeb::polish::cascade::PropertyValue`], which stores a
//!   property's value as one serialized-text string (see
//!   `cascade.rs`'s module docs) rather than Python's list of parsed
//!   `Value` tokens -- `content: "a" "b"` (two adjacent string tokens)
//!   is not distinguished from a single `"a" "b"` token; this is the
//!   same simplification `cascade.rs` already made, not a new one.

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::dom::{Dom, NodeId};
use crate::oeb::fonts3::parse_font_family;
use crate::oeb::polish::cascade::{self, PropertyValue, ResolvedStyles};

use super::container::Container;

/// Port of `widths`: stretch-keyword ordering, `ultra-condensed` (0) to
/// `ultra-expanded` (8), `normal` in the middle (4).
fn stretch_width(stretch: &str) -> i32 {
    const ORDER: &[&str] = &[
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
    ORDER
        .iter()
        .position(|&s| s == stretch)
        .map(|i| i as i32)
        .unwrap_or(4)
}

/// A `font`-shaped property bundle (family list, or already narrowed to
/// one family for an `@font-face` rule; weight/style/stretch), before
/// (or after) [`normalize_font_properties`]. Port of the `font`/`cssdict`
/// dicts `stats.py` passes around.
#[derive(Debug, Clone, Default)]
pub struct FontSpec {
    pub font_family: Vec<String>,
    pub font_weight: String,
    pub font_style: String,
    pub font_stretch: String,
}

/// Port of `normalize_font_properties`.
pub fn normalize_font_properties(font: &mut FontSpec) {
    let w = if font.font_weight.is_empty() {
        "normal".to_string()
    } else {
        font.font_weight.clone()
    };
    let w = match w.as_str() {
        "normal" => "400".to_string(),
        "bold" => "700".to_string(),
        _ => w,
    };
    font.font_weight = if matches!(
        w.as_str(),
        "100" | "200" | "300" | "400" | "500" | "600" | "700" | "800" | "900"
    ) {
        w
    } else {
        "400".to_string()
    };

    if !matches!(font.font_style.as_str(), "normal" | "italic" | "oblique") {
        font.font_style = "normal".to_string();
    }

    const STRETCH_KEYWORDS: &[&str] = &[
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
    if !STRETCH_KEYWORDS.contains(&font.font_stretch.as_str()) {
        font.font_stretch = "normal".to_string();
    }
}

/// Port of an `@font-face`-derived rule dict, after [`prepare_font_rule`]
/// (which narrows `font-family` to just its first entry -- matching
/// `frozenset(cssdict['font-family'][:1])` -- since a `@font-face` rule
/// declares exactly one family).
#[derive(Debug, Clone)]
pub struct FontFaceInfo {
    pub font_family: String,
    pub font_weight: String,
    pub font_style: String,
    pub font_stretch: String,
    pub width: i32,
    pub weight: i32,
    /// The container name of the embedded font file.
    pub src: String,
}

/// Port of `prepare_font_rule`.
fn prepare_font_rule(font_family: String, mut font: FontSpec, src: String) -> FontFaceInfo {
    normalize_font_properties(&mut font);
    FontFaceInfo {
        width: stretch_width(&font.font_stretch),
        weight: font.font_weight.parse().unwrap_or(400),
        font_family,
        font_weight: font.font_weight,
        font_style: font.font_style,
        font_stretch: font.font_stretch,
        src,
    }
}

/// Font-family names stats.py never treats as embeddable font
/// identities (generic families, plus `inherit`).
const BAD_FONTS: &[&str] = &[
    "serif",
    "sans-serif",
    "monospace",
    "cursive",
    "fantasy",
    "sansserif",
    "inherit",
];

/// Port of `skip_tags`: elements whose own text never contributes to
/// font-usage statistics.
fn is_skip_tag(tag: &str) -> bool {
    matches!(tag, "script" | "style" | "title" | "meta" | "link")
}

/// Port of `get_matching_rules`: `rules` narrowed to the ones matching
/// `font`, closest-match first per CSS Fonts Level 3 (stretch, then
/// style, then weight).
pub fn get_matching_rules<'a>(rules: &'a [FontFaceInfo], font: &FontSpec) -> Vec<&'a FontFaceInfo> {
    let family_lower: HashSet<String> = font.font_family.iter().map(|f| f.to_lowercase()).collect();
    let mut matches: Vec<&FontFaceInfo> = rules
        .iter()
        .rev()
        .filter(|r| family_lower.contains(&r.font_family))
        .collect();
    if matches.is_empty() {
        return Vec::new();
    }

    let width = stretch_width(&font.font_stretch);
    let min_dist = matches
        .iter()
        .map(|y| (width - y.width).abs())
        .min()
        .unwrap();
    let nearest: Vec<&FontFaceInfo> = matches
        .iter()
        .copied()
        .filter(|x| (width - x.width).abs() == min_dist)
        .collect();
    let narrowed: Vec<&FontFaceInfo> = if width <= 4 {
        nearest
            .iter()
            .copied()
            .filter(|f| f.width <= width)
            .collect()
    } else {
        nearest
            .iter()
            .copied()
            .filter(|f| f.width >= width)
            .collect()
    };
    matches = if narrowed.is_empty() {
        nearest
    } else {
        narrowed
    };

    let fs = if font.font_style.is_empty() {
        "normal"
    } else {
        font.font_style.as_str()
    };
    let order: &[&str] = match fs {
        "oblique" => &["oblique", "italic", "normal"],
        "normal" => &["normal", "oblique", "italic"],
        _ => &["italic", "oblique", "normal"],
    };
    for &q in order {
        let m: Vec<&FontFaceInfo> = matches
            .iter()
            .copied()
            .filter(|f| f.font_style == q)
            .collect();
        if !m.is_empty() {
            matches = m;
            break;
        }
    }

    let fw: i32 = if font.font_weight.is_empty() {
        400
    } else {
        font.font_weight.parse().unwrap_or(400)
    };
    let candidate_weights: Vec<i32> = if fw == 400 {
        vec![400, 500, 300, 200, 100, 600, 700, 800, 900]
    } else if fw == 500 {
        vec![500, 400, 300, 200, 100, 600, 700, 800, 900]
    } else if fw < 400 {
        let mut v = vec![fw];
        let mut x = fw - 100;
        while x >= 0 {
            v.push(x);
            x -= 100;
        }
        let mut x = fw + 100;
        while x < 1000 {
            v.push(x);
            x += 100;
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
        while x >= 0 {
            v.push(x);
            x -= 100;
        }
        v
    };
    for wt in candidate_weights {
        let m: Vec<&FontFaceInfo> = matches.iter().copied().filter(|f| f.weight == wt).collect();
        if !m.is_empty() {
            return m;
        }
    }
    Vec::new()
}

fn unicode_property_chars_only(text: &str) -> HashSet<char> {
    const EXCLUDE: &[char] = &['\n', '\r', '\t'];
    text.chars().filter(|c| !EXCLUDE.contains(c)).collect()
}

/// Port of `get_css_text`: the `content` value of the `before`/`after`
/// pseudo-element, with a wrapping pair of quotes stripped.
fn get_css_text(resolved: &ResolvedStyles, dom: &Dom, elem: NodeId, which: &str) -> String {
    let Some(val) = cascade::resolve_pseudo_property(
        &resolved.style_map,
        &resolved.pseudo_style_map,
        dom,
        elem,
        which,
        "content",
        false,
        false,
        false,
    ) else {
        return String::new();
    };
    let text = val.css_text;
    if text.len() > 2 && text.starts_with('"') && text.ends_with('"') {
        text[1..text.len() - 1].to_string()
    } else {
        String::new()
    }
}

const CAPS_VARIANTS: &[&str] = &[
    "smallcaps",
    "small-caps",
    "all-small-caps",
    "petite-caps",
    "all-petite-caps",
    "unicase",
];

/// Port of `get_element_text`. `for_pseudo`, when given, is the
/// pseudo-element name (`"first-letter"`/`"first-line"`) driving both
/// which text is gathered (the element's *full* rendered text, via
/// [`Dom::text_content`], rather than just its own direct text) and
/// which resolver (`resolve_pseudo_property`, bound to that pseudo)
/// computes `text-transform`/`font-variant`.
fn get_element_text(
    resolved: &ResolvedStyles,
    dom: &Dom,
    elem: NodeId,
    capitalize_pat: &regex::Regex,
    for_pseudo: Option<&str>,
) -> String {
    let mut ans = String::new();
    let before = get_css_text(resolved, dom, elem, "before");
    ans.push_str(&before);
    if let Some(_pseudo) = for_pseudo {
        ans.push_str(&dom.text_content(elem));
    } else {
        for &child in &dom.node(elem).children {
            if let crate::dom::NodeKind::Text(t) = &dom.node(child).kind {
                ans.push_str(t);
            }
        }
    }
    let after = get_css_text(resolved, dom, elem, "after");
    ans.push_str(&after);

    let (tt, fv) = if let Some(pseudo) = for_pseudo {
        let tt = cascade::resolve_pseudo_property(
            &resolved.style_map,
            &resolved.pseudo_style_map,
            dom,
            elem,
            pseudo,
            "text-transform",
            false,
            false,
            false,
        )
        .map(|v| v.css_text)
        .unwrap_or_else(|| "none".to_string());
        let fv = cascade::resolve_pseudo_property(
            &resolved.style_map,
            &resolved.pseudo_style_map,
            dom,
            elem,
            pseudo,
            "font-variant",
            false,
            false,
            false,
        )
        .map(|v| v.css_text)
        .unwrap_or_default();
        (tt, fv)
    } else {
        let tt = cascade::resolve_property(&resolved.style_map, dom, elem, "text-transform")
            .map(|v| v.css_text)
            .unwrap_or_else(|| "none".to_string());
        let fv = cascade::resolve_property(&resolved.style_map, dom, elem, "font-variant")
            .map(|v| v.css_text)
            .unwrap_or_default();
        (tt, fv)
    };
    if CAPS_VARIANTS.contains(&fv.as_str()) {
        ans.push_str(&ans.clone().to_uppercase());
    }
    if tt != "none" {
        match tt.as_str() {
            "uppercase" => ans = ans.to_uppercase(),
            "lowercase" => ans = ans.to_lowercase(),
            "capitalize" => {
                if let Some(m) = capitalize_pat.find(&ans) {
                    let upper = ans[m.start()..m.end()].to_uppercase();
                    ans.push_str(&upper);
                }
            }
            _ => {}
        }
    }
    ans
}

/// Port of `get_font_dict`.
fn get_font_dict(
    resolved: &ResolvedStyles,
    dom: &Dom,
    elem: NodeId,
    pseudo: Option<&str>,
) -> FontSpec {
    fn resolve(
        resolved: &ResolvedStyles,
        dom: &Dom,
        elem: NodeId,
        pseudo: Option<&str>,
        name: &str,
    ) -> Option<PropertyValue> {
        match pseudo {
            None => cascade::resolve_property(&resolved.style_map, dom, elem, name),
            Some(p) => cascade::resolve_pseudo_property(
                &resolved.style_map,
                &resolved.pseudo_style_map,
                dom,
                elem,
                p,
                name,
                false,
                false,
                false,
            ),
        }
    }
    let ff_text = resolve(resolved, dom, elem, pseudo, "font-family")
        .map(|v| v.css_text)
        .unwrap_or_default();
    let font_family = parse_font_family(&ff_text);
    let mut font = FontSpec {
        font_family,
        font_weight: resolve(resolved, dom, elem, pseudo, "font-weight")
            .map(|v| v.css_text)
            .unwrap_or_default(),
        font_style: resolve(resolved, dom, elem, pseudo, "font-style")
            .map(|v| v.css_text)
            .unwrap_or_default(),
        font_stretch: resolve(resolved, dom, elem, pseudo, "font-stretch")
            .map(|v| v.css_text)
            .unwrap_or_default(),
    };
    normalize_font_properties(&mut font);
    font
}

/// Port of the `frozenset(((k, ...) for k in font_keys))` key
/// `update_usage_for_embed` builds -- `font-family` narrowed to its
/// first entry, matching how [`FontFaceInfo`] narrows it too.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FontUsageKey {
    pub font_family: String,
    pub font_weight: String,
    pub font_style: String,
    pub font_stretch: String,
}

/// Port of `StatsCollector`.
pub struct StatsCollector {
    /// Font file container name -> every character rendered using it.
    pub font_stats: HashMap<String, HashSet<char>>,
    /// Spine item name -> (font spec actually used -> characters
    /// rendered with it). Only populated when `do_embed` is true (port
    /// of `update_usage_for_embed`'s early return otherwise).
    pub font_usage_map: HashMap<String, HashMap<FontUsageKey, HashSet<char>>>,
    /// Spine item name -> every non-generic font-family name referenced.
    pub font_spec_map: HashMap<String, HashSet<String>>,
    /// Spine item name -> every `@font-face` rule reachable from it.
    pub font_rule_map: HashMap<String, Vec<FontFaceInfo>>,
    /// Font file container name -> its `@font-face` rule.
    pub all_font_rules: HashMap<String, FontFaceInfo>,
}

impl StatsCollector {
    pub fn new(container: &mut Container, do_embed: bool) -> Result<Self> {
        let mut collector = StatsCollector {
            font_stats: HashMap::new(),
            font_usage_map: HashMap::new(),
            font_spec_map: HashMap::new(),
            font_rule_map: HashMap::new(),
            all_font_rules: HashMap::new(),
        };
        collector.collect_font_stats(container, do_embed)?;
        Ok(collector)
    }

    /// Port of `collect_font_face_rules`.
    fn collect_font_face_rules(
        container: &mut Container,
        processed: &mut HashMap<String, Vec<FontFaceInfo>>,
        spine_name: &str,
        sheet: &crate::css::Stylesheet,
        sheet_name: &str,
    ) -> Result<Vec<FontFaceInfo>> {
        if let Some(cached) = processed.get(sheet_name) {
            return Ok(cached.clone());
        }
        let mut sheet_rules = Vec::new();
        let mut counter = 0u64;
        let mut importing = std::collections::HashSet::new();
        let rules = cascade::iterrules(
            container,
            sheet_name,
            Some(&sheet.rules),
            &mut counter,
            Some(crate::css::RuleType::FontFace),
            &mut importing,
        )?;
        for (rule, rn, _idx) in rules {
            let crate::css::Rule::FontFace(decl) = rule else {
                continue;
            };
            let mut font = FontSpec::default();
            let mut src = None;
            for prop in cascade::iterdeclaration(&decl) {
                if prop.name.eq_ignore_ascii_case("font-family") {
                    font.font_family = parse_font_family(&prop.value)
                        .into_iter()
                        .map(|f| f.to_lowercase())
                        .collect();
                } else if prop.name.eq_ignore_ascii_case("font-weight") {
                    font.font_weight = prop.value.clone();
                } else if prop.name.eq_ignore_ascii_case("font-style") {
                    font.font_style = prop.value.clone();
                } else if prop.name.eq_ignore_ascii_case("font-stretch") {
                    font.font_stretch = prop.value.clone();
                } else if prop.name.eq_ignore_ascii_case("src") {
                    let mut v = prop.value.clone();
                    if let Some(rest) = v.strip_prefix("url(") {
                        v = rest.strip_suffix(')').unwrap_or(rest).to_string();
                    }
                    let v = super::fonts::unquote(&v);
                    let fname = container.href_to_name(v, Some(&rn));
                    if let Some(fname) = fname {
                        if container.has_name(&fname) {
                            src = Some(fname);
                        }
                    }
                }
            }
            let Some(src) = src else { continue };
            if font.font_family.is_empty() || BAD_FONTS.contains(&font.font_family[0].as_str()) {
                continue;
            }
            let first_family = font.font_family[0].clone();
            sheet_rules.push(prepare_font_rule(first_family, font, src));
        }
        if sheet_name != spine_name {
            processed.insert(sheet_name.to_string(), sheet_rules.clone());
        }
        Ok(sheet_rules)
    }

    #[allow(clippy::too_many_arguments)]
    fn get_element_font_usage(
        &mut self,
        resolved: &ResolvedStyles,
        dom: &Dom,
        elem: NodeId,
        capitalize_pat: &regex::Regex,
        first_letter_pat: &regex::Regex,
        font_face_rules: &[FontFaceInfo],
        do_embed: bool,
        spine_name: &str,
    ) {
        let text = get_element_text(resolved, dom, elem, capitalize_pat, None);
        if text.is_empty() {
            return;
        }

        let font = get_font_dict(resolved, dom, elem, None);
        let chars = unicode_property_chars_only(&text);
        self.update_usage_for_embed(&font, &chars, do_embed, spine_name);
        for rule in get_matching_rules(font_face_rules, &font) {
            self.font_stats
                .entry(rule.src.clone())
                .or_default()
                .extend(chars.iter().copied());
        }

        let applies = cascade::resolve_pseudo_property(
            &resolved.style_map,
            &resolved.pseudo_style_map,
            dom,
            elem,
            "first-letter",
            "font-family",
            false,
            true,
            false,
        );
        if applies.is_some() {
            let font = get_font_dict(resolved, dom, elem, Some("first-letter"));
            let text = get_element_text(resolved, dom, elem, capitalize_pat, Some("first-letter"));
            if let Some(m) = first_letter_pat.find(text.trim_start()) {
                let chars = unicode_property_chars_only(&text[m.start()..m.end()]);
                self.update_usage_for_embed(&font, &chars, do_embed, spine_name);
                for rule in get_matching_rules(font_face_rules, &font) {
                    self.font_stats
                        .entry(rule.src.clone())
                        .or_default()
                        .extend(chars.iter().copied());
                }
            }
        }

        let applies_line = cascade::resolve_pseudo_property(
            &resolved.style_map,
            &resolved.pseudo_style_map,
            dom,
            elem,
            "first-line",
            "font-family",
            false,
            true,
            true,
        );
        if applies_line.is_some() {
            let font = get_font_dict(resolved, dom, elem, Some("first-line"));
            let text = get_element_text(resolved, dom, elem, capitalize_pat, Some("first-line"));
            let chars = unicode_property_chars_only(&text);
            self.update_usage_for_embed(&font, &chars, do_embed, spine_name);
            for rule in get_matching_rules(font_face_rules, &font) {
                self.font_stats
                    .entry(rule.src.clone())
                    .or_default()
                    .extend(chars.iter().copied());
            }
        }
    }

    /// Port of `update_usage_for_embed`.
    fn update_usage_for_embed(
        &mut self,
        font: &FontSpec,
        chars: &HashSet<char>,
        do_embed: bool,
        spine_name: &str,
    ) {
        if !do_embed {
            return;
        }
        let ff_lower: Vec<String> = font.font_family.iter().map(|f| f.to_lowercase()).collect();
        if let Some(first) = ff_lower.first() {
            if !BAD_FONTS.contains(&first.as_str()) {
                let key = FontUsageKey {
                    font_family: first.clone(),
                    font_weight: font.font_weight.clone(),
                    font_style: font.font_style.clone(),
                    font_stretch: font.font_stretch.clone(),
                };
                self.font_usage_map
                    .entry(spine_name.to_string())
                    .or_default()
                    .entry(key)
                    .or_default()
                    .extend(chars.iter().copied());
            }
        }
        let spec = self
            .font_spec_map
            .entry(spine_name.to_string())
            .or_default();
        for ff in &font.font_family {
            if !ff.is_empty() && !BAD_FONTS.contains(&ff.to_lowercase().as_str()) {
                spec.insert(ff.clone());
            }
        }
    }

    fn get_font_usage(
        &mut self,
        container: &mut Container,
        spine_name: &str,
        resolved: &ResolvedStyles,
        font_face_rules: &[FontFaceInfo],
        do_embed: bool,
    ) -> Result<()> {
        container.ensure_parsed(spine_name)?;
        let capitalize_pat = capitalize_regex();
        let first_letter_pat = first_letter_regex();
        let dom = container.get_xhtml(spine_name)?;
        let Some(body) = dom.find_first_tag_global("body") else {
            return Ok(());
        };
        let elems = dom.preorder_elements(body);
        for elem in elems {
            if let Some(tag) = dom.tag(elem) {
                if is_skip_tag(tag) {
                    continue;
                }
            }
            self.get_element_font_usage(
                resolved,
                dom,
                elem,
                &capitalize_pat,
                &first_letter_pat,
                font_face_rules,
                do_embed,
                spine_name,
            );
        }
        Ok(())
    }

    /// Port of `collect_font_stats`.
    fn collect_font_stats(&mut self, container: &mut Container, do_embed: bool) -> Result<()> {
        let mut processed_sheets: HashMap<String, Vec<FontFaceInfo>> = HashMap::new();
        let spine: Vec<(String, bool)> = container.spine_names()?;
        for (name, _is_linear) in spine {
            let mut font_face_rules: Vec<FontFaceInfo> = Vec::new();
            let mut cb_error: Option<anyhow::Error> = None;
            let resolved = {
                let name_for_cb = name.clone();
                let processed = &mut processed_sheets;
                let rules_out = &mut font_face_rules;
                let cb_err = &mut cb_error;
                let mut sheet_cb = move |c: &mut Container,
                                         sheet: &crate::css::Stylesheet,
                                         sheet_name: &str|
                      -> Result<()> {
                    match StatsCollector::collect_font_face_rules(
                        c,
                        processed,
                        &name_for_cb,
                        sheet,
                        sheet_name,
                    ) {
                        Ok(rules) => {
                            rules_out.extend(rules);
                            Ok(())
                        }
                        Err(e) => {
                            *cb_err = Some(e);
                            Ok(())
                        }
                    }
                };
                cascade::resolve_styles(container, &name, Some(&mut sheet_cb))?
            };
            if let Some(e) = cb_error {
                return Err(e);
            }

            for rule in &font_face_rules {
                self.all_font_rules
                    .entry(rule.src.clone())
                    .or_insert_with(|| rule.clone());
                self.font_stats.entry(rule.src.clone()).or_default();
            }
            self.font_rule_map
                .entry(name.clone())
                .or_default()
                .extend(font_face_rules.clone());

            self.font_usage_map.entry(name.clone()).or_default();
            self.font_spec_map.entry(name.clone()).or_default();
            self.get_font_usage(container, &name, &resolved, &font_face_rules, do_embed)?;
        }
        Ok(())
    }
}

fn capitalize_regex() -> regex::Regex {
    regex::Regex::new(r"[\p{L}\p{N}]").unwrap()
}

fn first_letter_regex() -> regex::Regex {
    regex::Regex::new(r"^[\p{P}]*[\p{L}\p{N}]").unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_container(
        files: &[(&str, &str, &[u8])],
        spine: &[&str],
    ) -> (tempfile::TempDir, Container) {
        let dir = tempfile::tempdir().unwrap();
        let opf_path = dir.path().join("content.opf");
        let mut manifest_items = String::new();
        for (name, mt, content) in files {
            fs::write(dir.path().join(name), content).unwrap();
            manifest_items.push_str(&format!(
                r#"<item id="{name}" href="{name}" media-type="{mt}"/>"#
            ));
        }
        let spine_items: String = spine
            .iter()
            .map(|n| format!(r#"<itemref idref="{n}"/>"#))
            .collect();
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
    fn normalize_font_properties_maps_keywords_to_numbers() {
        let mut f = FontSpec {
            font_family: vec![],
            font_weight: "bold".to_string(),
            font_style: "weird".to_string(),
            font_stretch: "weird".to_string(),
        };
        normalize_font_properties(&mut f);
        assert_eq!(f.font_weight, "700");
        assert_eq!(f.font_style, "normal");
        assert_eq!(f.font_stretch, "normal");
    }

    #[test]
    fn get_matching_rules_prefers_exact_family_and_weight() {
        let rules = vec![
            FontFaceInfo {
                font_family: "x".to_string(),
                font_weight: "400".to_string(),
                font_style: "normal".to_string(),
                font_stretch: "normal".to_string(),
                width: 4,
                weight: 400,
                src: "X.otf".to_string(),
            },
            FontFaceInfo {
                font_family: "x".to_string(),
                font_weight: "700".to_string(),
                font_style: "normal".to_string(),
                font_stretch: "normal".to_string(),
                width: 4,
                weight: 700,
                src: "XB.otf".to_string(),
            },
        ];
        let font = FontSpec {
            font_family: vec!["x".to_string()],
            font_weight: "700".to_string(),
            font_style: "normal".to_string(),
            font_stretch: "normal".to_string(),
        };
        let m = get_matching_rules(&rules, &font);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].src, "XB.otf");
    }

    #[test]
    fn stats_collector_counts_characters_per_embedded_font() {
        let (_dir, mut container) = make_container(
            &[
                (
                    "index.html",
                    "application/xhtml+xml",
                    b"<html><head><style>\
                      @font-face { font-family: X; src: url(X.otf); font-weight: normal }\
                      @font-face { font-family: X; src: url(XB.otf); font-weight: bold }\
                      </style></head>\
                      <body><p style=\"font-family: X\">abc<b>def</b></p></body></html>",
                ),
                ("X.otf", "font/otf", b"fontbytes"),
                ("XB.otf", "font/otf", b"fontbytesbold"),
            ],
            &["index.html"],
        );
        let stats = StatsCollector::new(&mut container, true).unwrap();
        let mut abc: Vec<char> = stats
            .font_stats
            .get("X.otf")
            .unwrap()
            .iter()
            .copied()
            .collect();
        abc.sort_unstable();
        assert_eq!(abc, vec!['a', 'b', 'c']);
        let mut def: Vec<char> = stats
            .font_stats
            .get("XB.otf")
            .unwrap()
            .iter()
            .copied()
            .collect();
        def.sort_unstable();
        assert_eq!(def, vec!['d', 'e', 'f']);
        assert!(stats.font_spec_map.get("index.html").unwrap().contains("X"));
        assert!(stats.all_font_rules.contains_key("X.otf"));
        assert!(stats.all_font_rules.contains_key("XB.otf"));
    }

    #[test]
    fn stats_collector_handles_no_embedded_fonts() {
        let (_dir, mut container) = make_container(
            &[(
                "index.html",
                "application/xhtml+xml",
                b"<html><body><p>hi</p></body></html>",
            )],
            &["index.html"],
        );
        let stats = StatsCollector::new(&mut container, false).unwrap();
        assert!(stats.font_stats.is_empty());
        assert!(stats.font_usage_map.get("index.html").unwrap().is_empty());
    }
}
