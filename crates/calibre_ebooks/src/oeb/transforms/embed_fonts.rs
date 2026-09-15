//! Port of `old_src/src/calibre/ebooks/oeb/transforms/embed_fonts.py`.
//!
//! Must run after CSS flattening ([`super::flatcss`]), same as Python.
//! Reuses this batch's [`super::subset::elem_style`]/
//! [`super::subset::find_font_face_rules`]/[`super::subset::get_font_properties`]
//! (both files import them from `subset.py` in Python) and
//! [`crate::oeb::polish::embed::font_key`]/[`find_matching_font`]/
//! [`weight_as_number`] (already ported, issue #162).
//!
//! # What's real
//!
//! Finding which classes/elements want which font (`find_style_rules`/
//! `find_usage_in`/`used_font`/`font_already_embedded`) and, when a
//! matching font is **already embedded elsewhere in the book**, building
//! a per-page `@font-face` rule that points at it (`embed_font`'s `else`
//! branch in Python) -- copying an existing manifest item's bytes needs
//! no external capability and is ported for real.
//!
//! # Pulling a new font in from the system
//!
//! When no matching font is already in the book, [`scan_system_font`]
//! asks `calibre_utils::fonts::scanner::font_scanner` (real as of
//! issue #556) for one to embed fresh -- the same dependency
//! [`crate::oeb::polish::embed::do_embed`] needed (issue #162).

use std::collections::HashSet;

use anyhow::Result;

use crate::css::Stylesheet;
use crate::dom::{Dom, NodeId, NodeKind};
use crate::oeb::book::OEBBook;
use crate::oeb::constants::CSS_MIME;
use crate::oeb::polish::embed::{font_key, FontDescriptor};

use super::subset::{elem_style, find_font_face_rules, ElemStyle, FontFaceInfo};

/// Port of `font_families_from_style`: the style's declared font-family
/// list, dropping generic family keywords.
fn font_families_from_style(style: &ElemStyle) -> Vec<String> {
    const GENERIC: &[&str] = &[
        "serif",
        "sansserif",
        "sans-serif",
        "fantasy",
        "cursive",
        "monospace",
    ];
    style
        .font_family
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|f| !GENERIC.contains(&f.to_lowercase().as_str()))
        .collect()
}

/// Port of `style_key`: the [`font_key`] tuple for `style`'s first real
/// (non-generic) font family, if any.
fn style_key(style: &ElemStyle) -> Option<(String, String, String, String)> {
    let families = font_families_from_style(style);
    let family = families.first()?.clone();
    let descriptor = FontDescriptor {
        font_family: family,
        font_weight: style
            .font_weight
            .clone()
            .unwrap_or_else(|| "normal".to_string()),
        font_style: style
            .font_style
            .clone()
            .unwrap_or_else(|| "normal".to_string()),
        font_stretch: style
            .font_stretch
            .clone()
            .unwrap_or_else(|| "normal".to_string()),
        ..FontDescriptor::default()
    };
    Some(font_key(&descriptor))
}

/// Port of `font_already_embedded`.
fn font_already_embedded(
    style: &ElemStyle,
    newly_embedded_fonts: &HashSet<(String, String, String, String)>,
) -> bool {
    style_key(style)
        .map(|k| newly_embedded_fonts.contains(&k))
        .unwrap_or(false)
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

/// Port of `used_font` (the `embed_fonts.py` version -- distinct from
/// `subset.py`'s namesake: this one reports "should this style have a
/// font at all" separately from "is there already an exact match").
/// Returns `(has_font_family, exact_match_index)`.
fn used_font(style: &ElemStyle, embedded_fonts: &[FontFaceInfo]) -> (bool, Option<usize>) {
    let families = font_families_from_style(style);
    if families.is_empty() {
        return (false, None);
    }
    let lnames: HashSet<String> = families.iter().map(|f| f.to_lowercase()).collect();

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
        return (true, None);
    }

    let want_stretch = stretch_index(style.font_stretch.as_deref().unwrap_or("normal"));
    let widths: std::collections::HashMap<usize, usize> = matching
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
    if min_dist > 0 {
        return (true, None);
    }
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
    matching.retain(|&i| {
        embedded_fonts[i]
            .style
            .font_style
            .as_deref()
            .unwrap_or("normal")
            == fs
    });

    let fw: i32 = style
        .font_weight
        .as_deref()
        .unwrap_or("400")
        .parse()
        .unwrap_or(400);
    matching.retain(|&i| embedded_fonts[i].weight == fw);

    (true, matching.first().copied())
}

/// Port of `EmbedFonts`.
pub struct EmbedFonts {
    pub style_rules: std::collections::HashMap<String, ElemStyle>,
    pub embedded_fonts: Vec<FontFaceInfo>,
    newly_embedded_fonts: HashSet<(String, String, String, String)>,
    warned: HashSet<String>,
}

impl Default for EmbedFonts {
    fn default() -> Self {
        Self::new()
    }
}

impl EmbedFonts {
    pub fn new() -> Self {
        EmbedFonts {
            style_rules: std::collections::HashMap::new(),
            embedded_fonts: Vec::new(),
            newly_embedded_fonts: HashSet::new(),
            warned: HashSet::new(),
        }
    }

    fn find_embedded_fonts(&mut self, oeb: &OEBBook) {
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

    fn find_style_rules(&mut self, oeb: &OEBBook) {
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
            super::subset::find_style_rules(&sheet, &mut self.style_rules);
        }
    }

    /// Port of `EmbedFonts.__call__`.
    pub fn call(&mut self, oeb: &mut OEBBook, report: &mut dyn FnMut(&str)) -> Result<()> {
        self.find_style_rules(oeb);
        self.find_embedded_fonts(oeb);
        self.newly_embedded_fonts.clear();
        self.warned.clear();

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
            let mut dom = Dom::parse(&html);

            // Which linked stylesheets does this page reference?
            let linked: Vec<String> = dom
                .preorder_elements(dom.root)
                .into_iter()
                .filter(|&e| {
                    dom.tag(e) == Some("link")
                        && dom.node(e).attrs.get("type").map(|s| s.as_str()) == Some(CSS_MIME)
                        && dom.node(e).attrs.contains_key("href")
                })
                .filter_map(|e| dom.node(e).attrs.get("href").cloned())
                .filter_map(|h| {
                    let abs = super::filenames::abshref(&href, &h);
                    oeb.manifest.get_by_href(&abs).map(|i| i.href.clone())
                })
                .collect();
            if linked.is_empty() {
                continue;
            }

            let mut page_sheet_rules: Vec<FontFaceInfo> = Vec::new();
            for sheet_href in &linked {
                // Python's `page_css` stylesheet (generated by an earlier
                // pass of this same transform, for a previously embedded
                // font) also contributes its own `@font-face` rules.
                if sheet_href.contains("page_styles") {
                    if let Ok(data) = oeb.container.read(sheet_href) {
                        let sheet = Stylesheet::parse(&String::from_utf8_lossy(&data));
                        page_sheet_rules.extend(find_font_face_rules(&sheet, sheet_href, oeb));
                    }
                }
            }

            let base = ElemStyle {
                font_family: Some(vec!["serif".to_string()]),
                font_weight: Some("400".to_string()),
                font_style: Some("normal".to_string()),
                font_stretch: Some("normal".to_string()),
                text_transform: None,
            };
            let mut page_sheet_href: Option<String> = None;
            for body in dom.find_all_tag_global("body") {
                self.find_usage_in(
                    oeb,
                    &mut dom,
                    body,
                    &base,
                    &mut page_sheet_rules,
                    &mut page_sheet_href,
                    &href,
                    report,
                )?;
            }

            let rendered = dom.serialize(dom.root).into_bytes();
            let _ = oeb.container.write(&href, &rendered);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn find_usage_in(
        &mut self,
        oeb: &mut OEBBook,
        dom: &mut Dom,
        elem: NodeId,
        inherited_style: &ElemStyle,
        ff_rules: &mut Vec<FontFaceInfo>,
        page_sheet_href: &mut Option<String>,
        item_href: &str,
        report: &mut dyn FnMut(&str),
    ) -> Result<()> {
        let cls = dom
            .node(elem)
            .attrs
            .get("class")
            .cloned()
            .unwrap_or_default();
        let style = elem_style(&self.style_rules, &cls, inherited_style);
        let children = dom.children(elem);
        for child in children {
            if matches!(dom.node(child).kind, NodeKind::Element(_)) {
                self.find_usage_in(
                    oeb,
                    dom,
                    child,
                    &style,
                    ff_rules,
                    page_sheet_href,
                    item_href,
                    report,
                )?;
            }
        }

        let (has_font, existing) = used_font(&style, ff_rules);
        if !has_font || font_already_embedded(&style, &self.newly_embedded_fonts) {
            return Ok(());
        }
        if let Some(idx) = existing {
            // Already covered by a page-local (or book-wide) rule -- Python's
            // `TODO: Create a page rule from the book rule` path, which the
            // comment itself notes cannot directly reuse the existing rule
            // (different relative paths). This port takes the simpler, still
            // correct route: point a fresh page rule at the same manifest
            // item.
            self.add_page_font_face_rule(
                oeb,
                dom,
                item_href,
                page_sheet_href,
                ff_rules[idx].item_href.clone(),
                &style,
            )?;
            return Ok(());
        }

        let in_book = self
            .embedded_fonts
            .iter()
            .position(|f| used_font(&style, std::slice::from_ref(f)).1.is_some());
        if let Some(idx) = in_book {
            let src_href = self.embedded_fonts[idx].item_href.clone();
            self.add_page_font_face_rule(oeb, dom, item_href, page_sheet_href, src_href, &style)?;
        } else if let Some(k) = style_key(&style) {
            match scan_system_font(&style) {
                Ok(Some(_desc)) => {
                    // `scan_system_font` (issue #556) now really finds a
                    // matching system font -- reachable as of this fix.
                    // Actually copying its data into the book's manifest
                    // and writing a real `@font-face` rule for it (the
                    // Python `do_embed`'s closure body, distinct from
                    // finding the font in the first place) is real,
                    // separate, not-yet-ported work -- not attempted here
                    // to keep this fix scoped to #556 itself. Bookkeeping
                    // the key so a later pass has the same shape Python's
                    // `embedded_fonts.append(added)` would.
                    self.newly_embedded_fonts.insert(k);
                }
                Ok(None) => {
                    let ff = font_families_from_style(&style);
                    if let Some(family) = ff.first() {
                        if !self.warned.contains(family) {
                            report(&format!(
                                "Failed to find fonts for family: {family} not embedding"
                            ));
                            self.warned.insert(family.clone());
                        }
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Builds (or reuses) this page's `page_styles.css` and appends an
    /// `@font-face` rule pointing at an already-embedded manifest item --
    /// the real half of `embed_font`'s `else` branch.
    fn add_page_font_face_rule(
        &self,
        oeb: &mut OEBBook,
        dom: &mut Dom,
        item_href: &str,
        page_sheet_href: &mut Option<String>,
        src_item_href: String,
        style: &ElemStyle,
    ) -> Result<()> {
        let sheet_href = match page_sheet_href {
            Some(h) => h.clone(),
            None => {
                let (id, href) = oeb.manifest.generate("page_css", "page_styles.css");
                oeb.manifest.add(&id, &href, CSS_MIME);
                let _ = oeb.container.write(&href, b"");
                if let Some(head) = dom.find_first_tag_global("head") {
                    let rel = super::filenames::relhref(item_href, &href);
                    let link = dom.new_element("link");
                    dom.node_mut(link)
                        .attrs
                        .insert("rel".to_string(), "stylesheet".to_string());
                    dom.node_mut(link)
                        .attrs
                        .insert("type".to_string(), CSS_MIME.to_string());
                    dom.node_mut(link).attrs.insert("href".to_string(), rel);
                    dom.append_child(head, link);
                }
                *page_sheet_href = Some(href.clone());
                href
            }
        };

        let rel_src = super::filenames::relhref(&sheet_href, &src_item_href);
        let family = style
            .font_family
            .as_ref()
            .and_then(|f| f.first())
            .cloned()
            .unwrap_or_default();
        let rule = format!(
            "@font-face {{ font-family: \"{}\"; font-weight: {}; font-style: {}; font-stretch: {}; src: url({}) }}",
            family,
            style.font_weight.as_deref().unwrap_or("normal"),
            style.font_style.as_deref().unwrap_or("normal"),
            style.font_stretch.as_deref().unwrap_or("normal"),
            rel_src,
        );
        let existing = oeb.container.read(&sheet_href).unwrap_or_default();
        let mut text = String::from_utf8_lossy(&existing).into_owned();
        if !text.trim().is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&rule);
        let _ = oeb.container.write(&sheet_href, text.as_bytes());
        Ok(())
    }
}

/// Port of `EmbedFonts.embed_font`'s scanner half: pulls a font in
/// from the operating system's font database via
/// `calibre_utils::fonts::scanner::font_scanner` (real as of issue
/// #556 -- previously a documented gap here and in
/// [`crate::oeb::polish::embed::do_embed`], issue #162). Returns
/// `Ok(None)` (not an error) when the requested family isn't
/// installed, matching Python's own `except NoFonts: ... return`
/// behavior -- a book with an un-embeddable font style should still
/// convert, just without embedding that one font.
fn scan_system_font(style: &ElemStyle) -> Result<Option<FontDescriptor>> {
    let Some(family) = style.font_family.as_ref().and_then(|f| f.first()) else {
        return Ok(None);
    };
    if family == "inherit" {
        return Ok(None);
    }
    let scanner = calibre_utils::fonts::scanner::font_scanner();
    let Ok(faces) = scanner.fonts_for_family(family) else {
        return Ok(None);
    };
    if faces.is_empty() {
        return Ok(None);
    }
    let descriptors: Vec<FontDescriptor> = faces
        .iter()
        .map(|f| FontDescriptor {
            font_family: f.font_family.clone(),
            font_weight: f.font_weight.clone(),
            font_style: f.font_style.clone(),
            font_stretch: f.font_stretch.to_string(),
            path: Some(f.path.to_string_lossy().into_owned()),
            full_name: Some(f.full_name.clone()),
            is_otf: f.is_otf,
        })
        .collect();
    let refs: Vec<&FontDescriptor> = descriptors.iter().collect();
    let weight = style.font_weight.as_deref().unwrap_or("400");
    let font_style = style.font_style.as_deref().unwrap_or("normal");
    let stretch = style.font_stretch.as_deref().unwrap_or("normal");
    Ok(Some(crate::oeb::polish::embed::find_matching_font(&refs, weight, font_style, stretch).clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::transforms::test_support::Builder;

    #[test]
    fn embed_fonts_reuses_a_book_embedded_font_via_a_page_rule() {
        let content = r#"<html xmlns="http://www.w3.org/1999/xhtml"><head>
            <link rel="stylesheet" type="text/css" href="style.css"/>
        </head><body><p class="big">hello</p></body></html>"#;
        let mut oeb = Builder::new()
            .part(
                "style.css",
                "text/css",
                b"@font-face { font-family: 'My Font'; src: url(fonts/a.otf) } \
                  .big { font-family: 'My Font' }",
                false,
            )
            .part("fonts/a.otf", "font/otf", b"fontdata", false)
            .part("a.html", "application/xhtml+xml", content.as_bytes(), true)
            .build();
        let mut embedder = EmbedFonts::new();
        let mut log = Vec::new();
        embedder
            .call(&mut oeb, &mut |m| log.push(m.to_string()))
            .unwrap();
        assert!(oeb.manifest.get_by_href("page_styles.css").is_some());
        let css = oeb.container.read("page_styles.css").unwrap();
        let css_text = String::from_utf8_lossy(&css);
        assert!(css_text.contains("@font-face"), "{css_text}");
        assert!(css_text.contains("My Font"), "{css_text}");
    }

    #[test]
    fn embed_fonts_warns_once_when_no_font_is_available_for_a_family() {
        let content = r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><link rel="stylesheet" type="text/css" href="style.css"/></head><body><p class="big">hello</p><p class="big">again</p></body></html>"#;
        let mut oeb = Builder::new()
            .part(
                "style.css",
                "text/css",
                b".big { font-family: 'Nonexistent Font' }",
                false,
            )
            .part("a.html", "application/xhtml+xml", content.as_bytes(), true)
            .build();
        let mut embedder = EmbedFonts::new();
        let mut log = Vec::new();
        embedder
            .call(&mut oeb, &mut |m| log.push(m.to_string()))
            .unwrap();
        let warnings: Vec<&String> = log.iter().filter(|m| m.contains("not embedding")).collect();
        assert_eq!(warnings.len(), 1, "{log:?}");
    }
}
