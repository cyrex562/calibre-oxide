//! Port of `old_src/src/calibre/ebooks/oeb/polish/embed.py`.
//!
//! The font-matching *arithmetic* (`stretch_as_number`, `weight_as_number`,
//! `filter_by_stretch`/`filter_by_style`/`filter_by_weight`,
//! `find_matching_font`, `matching_rule`, `font_key`,
//! `format_fallback_match_report`) is plain value comparison over small
//! structs -- no CSS parser needed -- and is ported for real, matching
//! Python's CSS Fonts Level 3 matching algorithm
//! (<https://www.w3.org/TR/css-fonts-3/#font-style-matching>).
//!
//! Two things are genuinely out of scope:
//!
//! - **System font scanning** (`do_embed`, and `embed_font`'s fallback
//!   path): Python's `calibre.utils.fonts.scanner.font_scanner` enumerates
//!   fonts installed on the OS and calibre's own bundled font collection.
//!   No equivalent capability -- OS font enumeration, TrueType/OpenType
//!   name-table reading to build a searchable font database -- exists in
//!   this crate. `embed_font`'s *other* branch (reusing a font already
//!   embedded elsewhere in the book, matched via [`matching_rule_face`])
//!   needs none of that and is ported for real.
//! - **`embed_all_fonts`'s orchestration**: it walks `stats.font_usage_map`/
//!   `font_spec_map`/`font_rule_map`, which come from
//!   `oeb.polish.stats.StatsCollector` -- a different, not-yet-ported
//!   `polish/` file (outside this issue's 9-file scope; issue #162 only
//!   covers `cascade`/`download`/`embed`/`fonts`/`hyphenation`/`images`/
//!   `import_book`/`pretty`/`subset`). The per-font decision logic it
//!   would call (`matching_rule`, `font_key`, `embed_font`) is ported for
//!   real and independently testable; only the top-level walk that reads
//!   `StatsCollector`'s output is a placeholder.

use std::collections::HashSet;

use anyhow::Result;

use crate::mobi::dom::{Dom, NodeId};

use super::container::Container;

/// Port of the `font` dict shape `embed.py` operates on: `font-family`/
/// `font-weight`/`font-style`/`font-stretch` (CSS specification side)
/// plus the extra keys `calibre.utils.fonts.scanner` attaches to a
/// scanned system font (`path`/`full_name`/`is_otf`/`weight`/`text`).
#[derive(Debug, Clone)]
pub struct FontDescriptor {
    pub font_family: String,
    pub font_weight: String,
    pub font_style: String,
    pub font_stretch: String,
    pub path: Option<String>,
    pub full_name: Option<String>,
    pub is_otf: bool,
}

impl Default for FontDescriptor {
    fn default() -> Self {
        Self {
            font_family: String::new(),
            font_weight: "normal".to_string(),
            font_style: "normal".to_string(),
            font_stretch: "normal".to_string(),
            path: None,
            full_name: None,
            is_otf: false,
        }
    }
}

/// Port of a `@font-face` rule as tracked by `embed_all_fonts`: the CSS
/// specification side plus the embedded file's `src`/container name.
#[derive(Debug, Clone)]
pub struct FontFaceRule {
    pub font_family: String,
    pub font_weight: String,
    pub font_style: String,
    pub font_stretch: String,
    /// `url(...)`-wrapped href, matching Python's `rule['src']`.
    pub src: String,
    /// The container name of the embedded font file.
    pub name: String,
}

/// Port of `matching_rule`: the first `rules` entry whose weight/style/
/// stretch match `font` exactly and whose family matches
/// case-insensitively.
pub fn matching_rule<'a>(
    font: &FontDescriptor,
    rules: &'a [FontDescriptor],
) -> Option<&'a FontDescriptor> {
    let family = font.font_family.to_lowercase();
    rules.iter().find(|rule| {
        rule.font_style == font.font_style
            && rule.font_stretch == font.font_stretch
            && rule.font_weight == font.font_weight
            && rule.font_family.to_lowercase() == family
    })
}

/// [`matching_rule`]'s counterpart against already-embedded
/// `@font-face` rules (`all_font_rules` in Python).
pub fn matching_rule_face<'a>(
    font: &FontDescriptor,
    rules: &'a [FontFaceRule],
) -> Option<&'a FontFaceRule> {
    let family = font.font_family.to_lowercase();
    rules.iter().find(|rule| {
        rule.font_style == font.font_style
            && rule.font_stretch == font.font_stretch
            && rule.font_weight == font.font_weight
            && rule.font_family.to_lowercase() == family
    })
}

/// Port of `format_fallback_match_report`.
pub fn format_fallback_match_report(
    matched_font: &FontDescriptor,
    font_family: &str,
    css_font: &FontDescriptor,
    report: &mut dyn FnMut(&str),
) {
    let mut msg = format!(
        "Could not find a font in the \"{font_family}\" family exactly matching the CSS font specification, will embed a fallback font instead. CSS font specification:"
    );
    msg.push_str(&format!("\n\n* font-weight: {}", css_font.font_weight));
    msg.push_str(&format!("\n* font-style: {}", css_font.font_style));
    msg.push_str(&format!("\n* font-stretch: {}", css_font.font_stretch));
    msg.push_str("\n\nMatched font specification:");
    msg.push_str(&format!("\n{}", matched_font.path.as_deref().unwrap_or("")));
    msg.push_str(&format!(
        "\n\n* font-weight: {}",
        matched_font.font_weight.trim()
    ));
    msg.push_str(&format!(
        "\n* font-style: {}",
        matched_font.font_style.trim()
    ));
    msg.push_str(&format!(
        "\n* font-stretch: {}",
        matched_font.font_stretch.trim()
    ));
    report(&msg);
    report("");
}

/// Port of `stretch_as_number`.
pub fn stretch_as_number(val: &str) -> i32 {
    if let Ok(n) = val.parse::<i32>() {
        return n;
    }
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
        .position(|&s| s == val)
        .map(|i| i as i32)
        .unwrap_or(4)
}

/// Port of `filter_by_stretch`.
pub fn filter_by_stretch<'a>(fonts: &[&'a FontDescriptor], val: &str) -> Vec<&'a FontDescriptor> {
    let val_n = stretch_as_number(val);
    let stretch_map: Vec<i32> = fonts
        .iter()
        .map(|f| stretch_as_number(&f.font_stretch))
        .collect();
    let equal: Vec<&'a FontDescriptor> = fonts
        .iter()
        .enumerate()
        .filter(|&(i, _)| stretch_map[i] == val_n)
        .map(|(_, &f)| f)
        .collect();
    if !equal.is_empty() {
        return equal;
    }
    let condensed: Vec<usize> = (0..fonts.len()).filter(|&i| stretch_map[i] <= 4).collect();
    let expanded: Vec<usize> = (0..fonts.len()).filter(|&i| stretch_map[i] > 4).collect();
    let candidates: Vec<usize> = if val_n <= 4 {
        if !condensed.is_empty() {
            condensed
        } else {
            expanded
        }
    } else if !expanded.is_empty() {
        expanded
    } else {
        condensed
    };
    if candidates.is_empty() {
        return Vec::new();
    }
    let min_dist = candidates
        .iter()
        .map(|&i| (stretch_map[i] - val_n).abs())
        .min()
        .unwrap();
    candidates
        .into_iter()
        .filter(|&i| (stretch_map[i] - val_n).abs() == min_dist)
        .map(|i| fonts[i])
        .collect()
}

/// Port of `filter_by_style`.
pub fn filter_by_style<'a>(fonts: &[&'a FontDescriptor], val: &str) -> Vec<&'a FontDescriptor> {
    let order: &[&str] = match val {
        "italic" => &["italic", "oblique", "normal"],
        "oblique" => &["oblique", "italic", "normal"],
        _ => &["normal", "oblique", "italic"],
    };
    for &q in order {
        let ans: Vec<&'a FontDescriptor> = fonts
            .iter()
            .filter(|f| f.font_style == q)
            .copied()
            .collect();
        if !ans.is_empty() {
            return ans;
        }
    }
    fonts.to_vec()
}

/// Port of `weight_as_number`.
pub fn weight_as_number(wt: &str) -> i32 {
    if let Ok(n) = wt.parse::<i32>() {
        return n;
    }
    match wt {
        "bold" => 700,
        _ => 400,
    }
}

/// Port of `filter_by_weight`.
pub fn filter_by_weight<'a>(fonts: &[&'a FontDescriptor], val: &str) -> Vec<&'a FontDescriptor> {
    let val_n = weight_as_number(val);
    let weight_map: Vec<i32> = fonts
        .iter()
        .map(|f| weight_as_number(&f.font_weight))
        .collect();
    let equal: Vec<&'a FontDescriptor> = fonts
        .iter()
        .enumerate()
        .filter(|&(i, _)| weight_map[i] == val_n)
        .map(|(_, &f)| f)
        .collect();
    if !equal.is_empty() {
        return equal;
    }
    let mut rmap = std::collections::HashMap::new();
    for (i, &w) in weight_map.iter().enumerate() {
        rmap.insert(w, i);
    }
    let below: Vec<usize> = (0..fonts.len())
        .filter(|&i| weight_map[i] < val_n)
        .collect();
    let above: Vec<usize> = (0..fonts.len())
        .filter(|&i| weight_map[i] > val_n)
        .collect();
    let candidates: Vec<usize> = if val_n < 400 {
        if !below.is_empty() {
            below
        } else {
            above
        }
    } else if val_n > 500 {
        if !above.is_empty() {
            above
        } else {
            below
        }
    } else if val_n == 400 {
        if let Some(&i) = rmap.get(&500) {
            return vec![fonts[i]];
        }
        if !below.is_empty() {
            below
        } else {
            above
        }
    } else {
        if let Some(&i) = rmap.get(&400) {
            return vec![fonts[i]];
        }
        if !below.is_empty() {
            below
        } else {
            above
        }
    };
    if candidates.is_empty() {
        return Vec::new();
    }
    let min_dist = candidates
        .iter()
        .map(|&i| (weight_map[i] - val_n).abs())
        .min()
        .unwrap();
    candidates
        .into_iter()
        .filter(|&i| (weight_map[i] - val_n).abs() == min_dist)
        .map(|i| fonts[i])
        .collect()
}

/// Port of `find_matching_font`. See
/// <https://www.w3.org/TR/css-fonts-3/#font-style-matching>; as in
/// Python, unicode-range and `bolder`/`lighter` matching are not
/// implemented.
pub fn find_matching_font<'a>(
    fonts: &[&'a FontDescriptor],
    weight: &str,
    style: &str,
    stretch: &str,
) -> &'a FontDescriptor {
    let mut current: Vec<&'a FontDescriptor> = fonts.to_vec();
    current = filter_by_stretch(&current, stretch);
    if current.len() == 1 {
        return current[0];
    }
    current = filter_by_style(&current, style);
    if current.len() == 1 {
        return current[0];
    }
    current = filter_by_weight(&current, weight);
    current[0]
}

/// Port of `font_key`.
pub fn font_key(font: &FontDescriptor) -> (String, String, String, String) {
    (
        font.font_family.clone(),
        font.font_weight.clone(),
        font.font_style.clone(),
        font.font_stretch.clone(),
    )
}

/// Port of `do_embed`: copies a system font's file into the book and
/// returns its `@font-face` rule. Needs
/// `calibre.utils.fonts.scanner.font_scanner` -- see the module docs.
pub fn do_embed(
    _container: &mut Container,
    _font: &FontDescriptor,
    _report: &mut dyn FnMut(&str),
) -> Result<FontFaceRule> {
    todo!(
        "placeholder: needs calibre.utils.fonts.scanner.font_scanner (OS font \
         enumeration + calibre's bundled font collection, with \
         TrueType/OpenType name-table reading), which this crate has no \
         equivalent for -- see this module's docs"
    )
}

/// Port of `embed_font`. The already-embedded-elsewhere branch (a
/// matching rule found in `all_font_rules`) is real; the fallback that
/// scans the system font database needs [`do_embed`], see its docs.
pub fn embed_font(
    container: &mut Container,
    font: &FontDescriptor,
    all_font_rules: &[FontFaceRule],
    report: &mut dyn FnMut(&str),
    warned: &mut HashSet<String>,
) -> Result<Option<FontFaceRule>> {
    if let Some(rule) = matching_rule_face(font, all_font_rules) {
        let href = container.name_to_href(&rule.name, None);
        return Ok(Some(FontFaceRule {
            font_family: font.font_family.clone(),
            font_weight: rule.font_weight.clone(),
            font_style: rule.font_style.clone(),
            font_stretch: rule.font_stretch.clone(),
            src: format!("url({href})"),
            name: rule.name.clone(),
        }));
    }
    let _ = warned;
    do_embed(container, font, report).map(Some)
}

/// Port of `embed_all_fonts`. Needs `oeb.polish.stats.StatsCollector`
/// (`font_usage_map`/`font_spec_map`/`font_rule_map`/`all_font_rules`),
/// a different, not-yet-ported `polish/` file -- see the module docs.
/// The per-font decisions it would drive ([`matching_rule`],
/// [`font_key`], [`embed_font`]) are ported for real above.
pub fn embed_all_fonts(
    _container: &mut Container,
    _dom_by_spine_name: &[(String, Dom, NodeId)],
    _report: &mut dyn FnMut(&str),
) -> Result<bool> {
    todo!(
        "placeholder: needs oeb.polish.stats.StatsCollector's \
         font_usage_map/font_spec_map/font_rule_map/all_font_rules, which \
         comes from a different, not-yet-ported polish/ file (out of this \
         issue's 9-file scope) -- see this module's docs"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font(family: &str, weight: &str, style: &str, stretch: &str) -> FontDescriptor {
        FontDescriptor {
            font_family: family.to_string(),
            font_weight: weight.to_string(),
            font_style: style.to_string(),
            font_stretch: stretch.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn stretch_as_number_parses_numeric_and_keyword() {
        assert_eq!(stretch_as_number("5"), 5);
        assert_eq!(stretch_as_number("normal"), 4);
        assert_eq!(stretch_as_number("ultra-condensed"), 0);
        assert_eq!(stretch_as_number("ultra-expanded"), 8);
        assert_eq!(stretch_as_number("nonsense"), 4);
    }

    #[test]
    fn weight_as_number_parses_numeric_and_keyword() {
        assert_eq!(weight_as_number("300"), 300);
        assert_eq!(weight_as_number("bold"), 700);
        assert_eq!(weight_as_number("normal"), 400);
        assert_eq!(weight_as_number("nonsense"), 400);
    }

    #[test]
    fn filter_by_weight_prefers_500_over_nearby_when_querying_400_with_no_exact_match() {
        // No font is exactly weight 400, so the "equal" short-circuit
        // doesn't fire, and 500 is specifically preferred by the CSS
        // Fonts Level 3 matching rule for a 400 query.
        let f300 = font("F", "300", "normal", "normal");
        let f500 = font("F", "500", "normal", "normal");
        let fonts = vec![&f300, &f500];
        let result = filter_by_weight(&fonts, "normal");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].font_weight, "500");
    }

    #[test]
    fn filter_by_weight_returns_the_exact_match_when_one_exists() {
        let f400 = font("F", "400", "normal", "normal");
        let f500 = font("F", "500", "normal", "normal");
        let fonts = vec![&f400, &f500];
        let result = filter_by_weight(&fonts, "normal");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].font_weight, "400");
    }

    #[test]
    fn filter_by_weight_finds_nearest_when_no_exact_match() {
        let f300 = font("F", "300", "normal", "normal");
        let f800 = font("F", "800", "normal", "normal");
        let fonts = vec![&f300, &f800];
        let result = filter_by_weight(&fonts, "900");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].font_weight, "800");
    }

    #[test]
    fn filter_by_style_falls_back_to_oblique_then_normal() {
        let normal = font("F", "400", "normal", "normal");
        let fonts = vec![&normal];
        let result = filter_by_style(&fonts, "italic");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].font_style, "normal");
    }

    #[test]
    fn find_matching_font_narrows_to_a_single_font() {
        let a = font("F", "400", "normal", "normal");
        let b = font("F", "700", "italic", "condensed");
        let fonts = vec![&a, &b];
        let m = find_matching_font(&fonts, "700", "italic", "condensed");
        assert_eq!(m.font_weight, "700");
        assert_eq!(m.font_style, "italic");
    }

    #[test]
    fn matching_rule_is_case_insensitive_on_family() {
        let want = font("Times New Roman", "700", "normal", "normal");
        let rule = font("times new roman", "700", "normal", "normal");
        let rules = vec![rule.clone()];
        let m = matching_rule(&want, &rules);
        assert!(m.is_some());
    }

    #[test]
    fn font_key_is_a_stable_tuple() {
        let f = font("F", "700", "italic", "condensed");
        assert_eq!(
            font_key(&f),
            (
                "F".to_string(),
                "700".to_string(),
                "italic".to_string(),
                "condensed".to_string()
            )
        );
    }
}
