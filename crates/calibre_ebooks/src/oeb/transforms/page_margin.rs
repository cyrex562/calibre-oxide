//! Port of `old_src/src/calibre/ebooks/oeb/transforms/page_margin.py`.

use indexmap::IndexMap;

use crate::css::Stylesheet;
use crate::mobi::dom::{Dom, NodeId};
use crate::oeb::book::OEBBook;

const ADOBE_TEMPLATE_MEDIA_TYPES: &[&str] = &[
    "application/vnd.adobe-page-template+xml",
    "application/vnd.adobe.page-template+xml",
    "application/adobe-page-template+xml",
    "application/adobe.page-template+xml",
];

/// Port of `RemoveAdobeMargins`: strip margins from Adobe's page
/// templates.
pub struct RemoveAdobeMargins;

impl RemoveAdobeMargins {
    pub fn call(&self, oeb: &mut OEBBook) {
        let items: Vec<(String, String)> = oeb
            .manifest
            .iter()
            .map(|i| (i.href.clone(), i.media_type.clone()))
            .collect();
        for (href, media_type) in items {
            if !ADOBE_TEMPLATE_MEDIA_TYPES.contains(&media_type.as_str()) {
                continue;
            }
            let Ok(raw) = oeb.container.read(&href) else {
                continue;
            };
            let xml = String::from_utf8_lossy(&raw);
            let mut dom = Dom::parse(&xml);
            let mut changed = false;
            for el in dom.preorder_elements(dom.root) {
                for margin in ["margin-left", "margin-right", "margin-top", "margin-bottom"] {
                    if dom.node_mut(el).attrs.shift_remove(margin).is_some() {
                        changed = true;
                    }
                }
            }
            if changed {
                let rendered = dom.serialize(dom.root).into_bytes();
                let _ = oeb.container.write(&href, &rendered);
            }
        }
    }
}

fn level_of(dom: &Dom, mut elem: NodeId, body: NodeId) -> u32 {
    let mut ans = 1u32;
    // Defensive bound: `elem` is always a descendant of `body`, so this
    // terminates well before the guard fires; the guard exists only to
    // avoid a hang if the DOM is ever malformed.
    for _ in 0..10_000 {
        match dom.parent(elem) {
            Some(p) if p == body => return ans,
            Some(p) => {
                ans += 1;
                elem = p;
            }
            None => return ans,
        }
    }
    ans
}

/// Returns `(margin_left, margin_right)` for `class`'s rule in
/// `selector_map`, or `Err(())` if that rule has a negative
/// `text-indent` (port of the `NegativeTextIndent` signal, which aborts
/// processing of the whole level it was found in).
fn get_margins(
    class: Option<&str>,
    selector_map: &IndexMap<String, crate::css::StyleDeclarationBlock>,
) -> Result<(String, String), ()> {
    let Some(cls) = class else {
        return Ok((String::new(), String::new()));
    };
    let Some(style) = selector_map.get(&format!(".{cls}")) else {
        return Ok((String::new(), String::new()));
    };
    let ti = style.get_property_value("text-indent");
    if ti.starts_with('-') {
        return Err(());
    }
    Ok((
        style.get_property_value("margin-left").to_string(),
        style.get_property_value("margin-right").to_string(),
    ))
}

/// Port of `RemoveFakeMargins.analyze_stats`: `Some((value, count))` for
/// the majority (>95%) non-empty/non-zero margin value, if the stats
/// support removing it.
fn analyze_stats(stats: &IndexMap<String, u32>) -> Option<(String, u32)> {
    if stats.is_empty() {
        return None;
    }
    let (most_common, count) = stats.iter().max_by_key(|(_, c)| **c)?;
    if most_common.is_empty() || most_common == "0" {
        return None;
    }
    let total: u32 = stats.values().sum();
    if total == 0 {
        return None;
    }
    if (*count as f64) / (total as f64) > 0.95 {
        Some((most_common.clone(), *count))
    } else {
        None
    }
}

/// Port of `RemoveFakeMargins`: remove left/right margins from
/// paragraph/div elements when the same margin is specified on almost
/// all elements at that structural level, since that's almost always an
/// artifact of an authoring tool rather than intentional formatting.
/// Must run after CSS flattening (there must be a single main
/// stylesheet with per-class rules for this to find anything).
pub struct RemoveFakeMargins;

impl RemoveFakeMargins {
    pub fn call(&self, oeb: &mut OEBBook, remove_fake_margins: bool) {
        if !remove_fake_margins {
            return;
        }
        let Some(sheet_href) = oeb.manifest.main_stylesheet().map(|i| i.href.clone()) else {
            return;
        };
        let Ok(raw) = oeb.container.read(&sheet_href) else {
            return;
        };
        let css_text = String::from_utf8_lossy(&raw).into_owned();
        let mut sheet = Stylesheet::parse(&css_text);

        let mut selector_map: IndexMap<String, crate::css::StyleDeclarationBlock> = IndexMap::new();
        for rule in sheet.style_rules() {
            selector_map.insert(rule.selector_text.trim().to_string(), rule.style.clone());
        }

        let docs: Vec<(String, Dom)> = oeb
            .spine
            .iter()
            .filter_map(|s| oeb.manifest.get_by_id(&s.idref))
            .filter_map(|item| {
                oeb.container.read(&item.href).ok().map(|raw| {
                    (
                        item.href.clone(),
                        Dom::parse(&String::from_utf8_lossy(&raw)),
                    )
                })
            })
            .collect();

        let mut levels: IndexMap<String, Vec<Option<String>>> = IndexMap::new();
        for (_, dom) in &docs {
            let Some(body) = dom.find_first_tag_global("body") else {
                continue;
            };
            for p in dom.preorder_elements(body) {
                if p == body {
                    continue;
                }
                let tag = dom.tag(p).unwrap_or("");
                if tag != "p" && tag != "div" {
                    continue;
                }
                let level = level_of(dom, p, body);
                if tag == "div" && level < 3 {
                    let descendant_paras = dom
                        .preorder_elements(p)
                        .into_iter()
                        .filter(|&e| e != p && matches!(dom.tag(e), Some("p") | Some("div")))
                        .count();
                    if descendant_paras < 5 {
                        continue;
                    }
                }
                let key = format!("{tag}_{level}");
                let cls = dom.node(p).attrs.get("class").cloned();
                levels.entry(key).or_default().push(cls);
            }
        }
        levels.retain(|k, v| {
            let num = v.len();
            let (tag, level_str) = k.split_once('_').unwrap_or((k.as_str(), "0"));
            let level: u32 = level_str.parse().unwrap_or(0);
            if tag == "p" && num < 25 {
                return false;
            }
            if tag == "div" && level > 2 && num < 25 {
                return false;
            }
            true
        });

        for (_, elems) in &levels {
            let mut left_stats: IndexMap<String, u32> = IndexMap::new();
            let mut right_stats: IndexMap<String, u32> = IndexMap::new();
            let mut negative_indent = false;
            for cls in elems {
                match get_margins(cls.as_deref(), &selector_map) {
                    Ok((lm, rm)) => {
                        *left_stats.entry(lm).or_insert(0) += 1;
                        *right_stats.entry(rm).or_insert(0) += 1;
                    }
                    Err(()) => {
                        negative_indent = true;
                        break;
                    }
                }
            }
            if negative_indent {
                continue;
            }
            let remove_left = analyze_stats(&left_stats);
            let remove_right = analyze_stats(&right_stats);
            if remove_left.is_none() && remove_right.is_none() {
                continue;
            }
            for cls in elems {
                let Some(cls) = cls else { continue };
                let selector = format!(".{cls}");
                if let Some((mcl, _)) = &remove_left {
                    let (lm, _) = get_margins(Some(cls), &selector_map).unwrap_or_default();
                    if &lm == mcl {
                        if let Some(rule) = sheet
                            .style_rules_mut()
                            .find(|r| r.selector_text.trim() == selector)
                        {
                            rule.style.remove_property("margin-left");
                        }
                    }
                }
                if let Some((mcr, _)) = &remove_right {
                    let (_, rm) = get_margins(Some(cls), &selector_map).unwrap_or_default();
                    if &rm == mcr {
                        if let Some(rule) = sheet
                            .style_rules_mut()
                            .find(|r| r.selector_text.trim() == selector)
                        {
                            rule.style.remove_property("margin-right");
                        }
                    }
                }
            }
        }

        let _ = oeb
            .container
            .write(&sheet_href, sheet.to_css_text().as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::transforms::test_support::Builder;

    #[test]
    fn removes_adobe_page_template_margins() {
        // Note: the root element deliberately avoids the real HTML5
        // `<template>` tag name, which html5ever gives special
        // "template contents" fragment handling that this crate's `Dom`
        // preorder walk doesn't reach into.
        let mut oeb = Builder::new()
            .part(
                "template.xml",
                "application/vnd.adobe-page-template+xml",
                br#"<adobe-template><region margin-left="10pt" margin-top="5pt"/></adobe-template>"#,
                false,
            )
            .build();
        RemoveAdobeMargins.call(&mut oeb);
        let raw = oeb.container.read("template.xml").unwrap();
        let xml = String::from_utf8_lossy(&raw);
        assert!(!xml.contains("margin-left"), "{xml}");
        assert!(!xml.contains("margin-top"), "{xml}");
    }

    #[test]
    fn removes_majority_margin_when_almost_all_paragraphs_share_it() {
        let mut body = String::new();
        for i in 0..30 {
            body.push_str(&format!("<p class=\"c\">para {i}</p>"));
        }
        let css = ".c { margin-left: 2em; margin-right: 0; text-indent: 1em; }";
        let mut oeb = Builder::new()
            .page("a.html", &body)
            .part("main.css", "text/css", css.as_bytes(), false)
            .build();
        RemoveFakeMargins.call(&mut oeb, true);
        let raw = oeb.container.read("main.css").unwrap();
        let out = String::from_utf8_lossy(&raw);
        assert!(!out.contains("margin-left"), "{out}");
    }

    #[test]
    fn does_nothing_when_disabled() {
        let css = ".c { margin-left: 2em; }";
        let mut oeb = Builder::new()
            .page("a.html", "<p class=\"c\">x</p>")
            .part("main.css", "text/css", css.as_bytes(), false)
            .build();
        RemoveFakeMargins.call(&mut oeb, false);
        let raw = oeb.container.read("main.css").unwrap();
        let out = String::from_utf8_lossy(&raw);
        assert!(out.contains("margin-left"), "{out}");
    }
}
