//! Port of `old_src/src/calibre/ebooks/oeb/polish/fonts.py`.
//!
//! Every function in this file except [`unquote`] needed real CSS
//! parsing: reading/mutating `font-family` declarations inside parsed
//! stylesheets, `<style>` tags, and `style=""` attributes. Issue #164
//! added that ([`crate::css`]), so this file is now real end to end,
//! with one deliberate, documented narrowing shared by
//! [`font_family_data_from_declaration`]/[`change_font_in_declaration`]:
//! neither handles the `font` shorthand property (`font: bold 12px
//! Georgia, serif`) -- only `font-family` declared directly. Python
//! extracts a family list from `font` too, via
//! `tinycss.fonts3.parse_font`/`normalize_font` (the full CSS Fonts
//! Level 3 `font` shorthand grammar: style/variant/weight/stretch/
//! size/line-height/family in a fixed token order -- see
//! `old_src/src/tinycss/fonts3.py`'s `parse_font`). This crate ports
//! only `parse_font_family`/`serialize_font_family` (issue #164's own
//! scoping note: "simple string parsing, not full CSS" -- see
//! [`crate::oeb::fonts3`]), not that larger shorthand grammar, so a
//! `font-family` set *only* via the `font` shorthand is invisible to
//! these two functions. This is a real, working simplification (not a
//! `todo!()`): declaring `font-family` directly is by far the more
//! common real-world pattern, and every other function in this file
//! (which does not need `font` shorthand parsing) is unaffected.

use std::collections::HashMap;

use anyhow::Result;

use crate::css::{Rule, Stylesheet};
use crate::dom::{Dom, NodeId};
use crate::oeb::constants::{OEB_DOCS, OEB_STYLES};
use crate::oeb::fonts3::{parse_font_family, serialize_font_family};

use super::container::Container;

/// Port of `unquote`: strips one layer of matching leading/trailing
/// quote characters, if present.
pub fn unquote(x: &str) -> &str {
    let bytes = x.as_bytes();
    if x.len() > 1 && bytes[0] == bytes[x.len() - 1] && matches!(bytes[0], b'"' | b'\'') {
        &x[1..x.len() - 1]
    } else {
        x
    }
}

/// Port of `font_family_data_from_declaration`. See the module docs for
/// why the `font` shorthand branch is not handled.
pub fn font_family_data_from_declaration(
    style: &crate::css::StyleDeclarationBlock,
    families: &mut HashMap<String, bool>,
) {
    if let Some(ff) = style.get_property("font-family") {
        for f in parse_font_family(&ff.value) {
            families.entry(f).or_insert(false);
        }
    }
}

/// Port of `font_family_data_from_sheet`. Mirrors Python's shallow
/// `for rule in sheet.cssRules` -- rules nested inside `@media` are not
/// visited, matching the original (not a simplification this port
/// introduced).
pub fn font_family_data_from_sheet(sheet: &Stylesheet, families: &mut HashMap<String, bool>) {
    for rule in &sheet.rules {
        match rule {
            Rule::Style(sr) => font_family_data_from_declaration(&sr.style, families),
            Rule::FontFace(decl) => {
                if let Some(ff) = decl.get_property("font-family") {
                    for f in parse_font_family(&ff.value) {
                        families.insert(f, true);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Port of `font_family_data`: every font-family name referenced
/// anywhere in the book (in stylesheets, `<style>` tags, and `style=""`
/// attributes), and whether it's declared via `@font-face` (embedded).
pub fn font_family_data(container: &mut Container) -> Result<HashMap<String, bool>> {
    let mut families = HashMap::new();
    let names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (name, mt) in names {
        if OEB_STYLES.contains(&mt.as_str()) {
            let sheet = container.parsed_stylesheet(&name)?;
            font_family_data_from_sheet(&sheet, &mut families);
        } else if OEB_DOCS.contains(&mt.as_str()) {
            container.ensure_parsed(&name)?;
            let dom = container.get_xhtml(&name)?;
            for id in dom.preorder_elements(dom.root) {
                if dom.tag(id) == Some("style") && style_tag_is_css(dom, id) {
                    let text = dom.text_content(id);
                    if !text.trim().is_empty() {
                        font_family_data_from_sheet(&Stylesheet::parse(&text), &mut families);
                    }
                }
                if let Some(style) = dom.node(id).attrs.get("style") {
                    if !style.trim().is_empty() {
                        let decl = crate::css::parser::parse_declaration_list(style);
                        font_family_data_from_declaration(&decl, &mut families);
                    }
                }
            }
        }
    }
    Ok(families)
}

pub(crate) fn style_tag_is_css(dom: &Dom, id: NodeId) -> bool {
    dom.node(id)
        .attrs
        .get("type")
        .map(|t| t.eq_ignore_ascii_case("text/css"))
        .unwrap_or(true)
}

/// Port of `change_font_in_declaration`. See the module docs for why the
/// `font` shorthand branch is not handled.
pub fn change_font_in_declaration(
    style: &mut crate::css::StyleDeclarationBlock,
    old_name: &str,
    new_name: Option<&str>,
) -> bool {
    let mut changed = false;
    if let Some(ff) = style.get_property("font-family").cloned() {
        let fams = parse_font_family(&ff.value);
        let nfams: Vec<String> = fams
            .iter()
            .filter_map(|x| {
                if x == old_name {
                    new_name.map(|s| s.to_string())
                } else {
                    Some(x.clone())
                }
            })
            .collect();
        if fams != nfams {
            if nfams.is_empty() {
                style.remove_property("font-family");
            } else {
                style.set_property("font-family", serialize_font_family(&nfams), ff.important);
            }
            changed = true;
        }
    }
    changed
}

/// Port of `remove_embedded_font`: removes the `@font-face` rule at
/// `rule_index` from `sheet` and, if it pointed at a font file embedded
/// in the book, removes that file too.
pub fn remove_embedded_font(
    container: &mut Container,
    sheet: &mut Stylesheet,
    rule_index: usize,
    sheet_name: &str,
) -> Result<()> {
    let mut src = None;
    if let Some(Rule::FontFace(decl)) = sheet.rules.get(rule_index) {
        if let Some(src_prop) = decl.get_property("src") {
            let mut v = src_prop.value.clone();
            if let Some(rest) = v.strip_prefix("url(") {
                v = rest.strip_suffix(')').unwrap_or(rest).to_string();
            }
            src = Some(v);
        }
    }
    if rule_index < sheet.rules.len() {
        sheet.rules.remove(rule_index);
    }
    if let Some(src) = src {
        if !src.is_empty() {
            let src = unquote(&src);
            if let Some(name) = container.href_to_name(src, Some(sheet_name)) {
                if container.has_name(&name) {
                    container.remove_item(&name, true)?;
                }
            }
        }
    }
    Ok(())
}

/// Port of `change_font_in_sheet`.
pub fn change_font_in_sheet(
    container: &mut Container,
    sheet: &mut Stylesheet,
    old_name: &str,
    new_name: Option<&str>,
    sheet_name: &str,
) -> Result<bool> {
    let mut changed = false;
    let mut removals = Vec::new();
    for (i, rule) in sheet.rules.iter_mut().enumerate() {
        match rule {
            Rule::Style(sr) => {
                if change_font_in_declaration(&mut sr.style, old_name, new_name) {
                    changed = true;
                }
            }
            Rule::FontFace(decl) => {
                if let Some(ff) = decl.get_property("font-family") {
                    if parse_font_family(&ff.value).iter().any(|f| f == old_name) {
                        changed = true;
                        removals.push(i);
                    }
                }
            }
            _ => {}
        }
    }
    for i in removals.into_iter().rev() {
        remove_embedded_font(container, sheet, i, sheet_name)?;
    }
    Ok(changed)
}

/// Replaces `id`'s entire child list with a single text node containing
/// `text` -- what a `<style>` tag's content reduces to (real-world
/// `<style>` tags never have element children).
pub(crate) fn set_dom_element_text_only(dom: &mut Dom, id: NodeId, text: &str) {
    let children: Vec<NodeId> = dom.node(id).children.clone();
    for c in children {
        dom.detach(c);
    }
    let tid = dom.new_text(text);
    dom.append_child(id, tid);
}

/// Port of `change_font`: renames (or removes) a font-family everywhere
/// it's referenced in the book, removing the embedded font file if the
/// old name pointed at one.
pub fn change_font(
    container: &mut Container,
    old_name: &str,
    new_name: Option<&str>,
) -> Result<bool> {
    let mut changed = false;
    let names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (name, mt) in names {
        if OEB_STYLES.contains(&mt.as_str()) {
            let mut sheet = container.parsed_stylesheet(&name)?;
            if change_font_in_sheet(container, &mut sheet, old_name, new_name, &name)? {
                container.set_css_text(&name, sheet.to_css_text());
                container.dirty(&name);
                changed = true;
            }
        } else if OEB_DOCS.contains(&mt.as_str()) {
            container.ensure_parsed(&name)?;

            // Collect owned copies before doing any &mut Container work
            // (change_font_in_sheet -> remove_embedded_font needs it),
            // so no Dom borrow is held across those calls.
            let (style_tags, style_attrs) = {
                let dom = container.get_xhtml(&name)?;
                let mut style_tags = Vec::new();
                let mut style_attrs = Vec::new();
                for id in dom.preorder_elements(dom.root) {
                    if dom.tag(id) == Some("style") && style_tag_is_css(dom, id) {
                        let text = dom.text_content(id);
                        if !text.trim().is_empty() {
                            style_tags.push((id, text));
                        }
                    }
                    if let Some(style) = dom.node(id).attrs.get("style") {
                        if !style.is_empty() {
                            style_attrs.push((id, style.clone()));
                        }
                    }
                }
                (style_tags, style_attrs)
            };

            let mut style_tag_updates = Vec::new();
            for (id, text) in style_tags {
                let mut sheet = Stylesheet::parse(&text);
                if change_font_in_sheet(container, &mut sheet, old_name, new_name, &name)? {
                    style_tag_updates.push((id, sheet.to_css_text()));
                    changed = true;
                }
            }
            let mut style_attr_updates = Vec::new();
            for (id, text) in style_attrs {
                let mut decl = crate::css::parser::parse_declaration_list(&text);
                if change_font_in_declaration(&mut decl, old_name, new_name) {
                    let new_text = decl
                        .to_css_text(" ")
                        .trim()
                        .trim_end_matches(';')
                        .trim()
                        .to_string();
                    style_attr_updates.push((id, new_text));
                    changed = true;
                }
            }

            if !style_tag_updates.is_empty() || !style_attr_updates.is_empty() {
                let dom = container.get_xhtml_mut(&name)?;
                for (id, text) in style_tag_updates {
                    set_dom_element_text_only(dom, id, &text);
                }
                for (id, text) in style_attr_updates {
                    if text.is_empty() {
                        dom.node_mut(id).attrs.shift_remove("style");
                    } else {
                        dom.node_mut(id).attrs.insert("style".to_string(), text);
                    }
                }
                container.dirty(&name);
            }
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn unquote_strips_matching_quotes() {
        assert_eq!(unquote("\"My Font\""), "My Font");
        assert_eq!(unquote("'My Font'"), "My Font");
        assert_eq!(unquote("My Font"), "My Font");
        assert_eq!(unquote("\"unterminated"), "\"unterminated");
        assert_eq!(unquote(""), "");
        assert_eq!(unquote("\""), "\"");
    }

    #[test]
    fn font_family_data_from_sheet_marks_font_face_families_as_embedded() {
        let sheet = Stylesheet::parse(
            "p { font-family: Georgia, serif } @font-face { font-family: MyFont; src: url(f.otf) }",
        );
        let mut families = HashMap::new();
        font_family_data_from_sheet(&sheet, &mut families);
        assert_eq!(families.get("Georgia"), Some(&false));
        assert_eq!(families.get("serif"), Some(&false));
        assert_eq!(families.get("MyFont"), Some(&true));
    }

    #[test]
    fn change_font_in_declaration_renames_and_removes() {
        let mut style = crate::css::parser::parse_declaration_list("font-family: Old, serif");
        assert!(change_font_in_declaration(&mut style, "Old", Some("New")));
        assert_eq!(style.get_property_value("font-family"), "New, serif");

        let mut style2 = crate::css::parser::parse_declaration_list("font-family: Old");
        assert!(change_font_in_declaration(&mut style2, "Old", None));
        assert!(style2.get_property("font-family").is_none());

        let mut style3 = crate::css::parser::parse_declaration_list("font-family: Other");
        assert!(!change_font_in_declaration(&mut style3, "Old", Some("New")));
    }

    fn make_container(files: &[(&str, &str, &[u8])]) -> (tempfile::TempDir, Container) {
        let dir = tempfile::tempdir().unwrap();
        let opf_path = dir.path().join("content.opf");
        let mut manifest_items = String::new();
        for (name, mt, content) in files {
            fs::write(dir.path().join(name), content).unwrap();
            manifest_items.push_str(&format!(
                r#"<item id="{name}" href="{name}" media-type="{mt}"/>"#
            ));
        }
        let opf = format!(
            r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>T</dc:title><dc:identifier id="bookid">x</dc:identifier></metadata>
  <manifest>{manifest_items}</manifest>
  <spine></spine>
</package>"#
        );
        fs::write(&opf_path, opf).unwrap();
        let container = Container::open(dir.path(), &opf_path).unwrap();
        (dir, container)
    }

    #[test]
    fn font_family_data_walks_stylesheets_style_tags_and_style_attrs() {
        let (_dir, mut container) = make_container(&[
            ("style.css", "text/css", b"h1 { font-family: Georgia }"),
            (
                "index.html",
                "application/xhtml+xml",
                b"<html><head><style>p { font-family: Arial }</style></head>\
                  <body><span style=\"font-family: Verdana\">x</span></body></html>",
            ),
        ]);
        let families = font_family_data(&mut container).unwrap();
        assert_eq!(families.get("Georgia"), Some(&false));
        assert_eq!(families.get("Arial"), Some(&false));
        assert_eq!(families.get("Verdana"), Some(&false));
    }

    #[test]
    fn change_font_updates_css_file_and_marks_it_dirty() {
        let (_dir, mut container) =
            make_container(&[("style.css", "text/css", b"h1 { font-family: Old, serif }")]);
        let changed = change_font(&mut container, "Old", Some("New")).unwrap();
        assert!(changed);
        assert!(container.dirtied.contains("style.css"));
        let sheet = container.parsed_stylesheet("style.css").unwrap();
        let r = sheet.style_rules().next().unwrap();
        assert_eq!(r.style.get_property_value("font-family"), "New, serif");
    }

    #[test]
    fn change_font_updates_style_tag_and_style_attribute_in_xhtml() {
        let (_dir, mut container) = make_container(&[(
            "index.html",
            "application/xhtml+xml",
            b"<html><head><style>p { font-family: Old }</style></head>\
              <body><span style=\"font-family: Old, serif\">x</span></body></html>",
        )]);
        let changed = change_font(&mut container, "Old", Some("New")).unwrap();
        assert!(changed);
        assert!(container.dirtied.contains("index.html"));
        let dom = container.get_xhtml("index.html").unwrap();
        let style_tag = dom.find_first_tag_global("style").unwrap();
        let sheet = Stylesheet::parse(&dom.text_content(style_tag));
        assert_eq!(
            sheet
                .style_rules()
                .next()
                .unwrap()
                .style
                .get_property_value("font-family"),
            "New"
        );
        let span = dom.find_first_tag_global("span").unwrap();
        assert_eq!(
            dom.node(span).attrs.get("style").map(|s| s.as_str()),
            Some("font-family: New, serif")
        );
    }

    #[test]
    fn change_font_removes_style_attribute_when_it_becomes_empty() {
        let (_dir, mut container) = make_container(&[(
            "index.html",
            "application/xhtml+xml",
            b"<html><body><span style=\"font-family: Old\">x</span></body></html>",
        )]);
        change_font(&mut container, "Old", None).unwrap();
        let dom = container.get_xhtml("index.html").unwrap();
        let span = dom.find_first_tag_global("span").unwrap();
        assert!(!dom.node(span).attrs.contains_key("style"));
    }

    #[test]
    fn change_font_removes_embedded_font_file_referenced_by_font_face() {
        let (_dir, mut container) = make_container(&[
            (
                "style.css",
                "text/css",
                b"@font-face { font-family: Old; src: url(old.otf) }",
            ),
            ("old.otf", "font/otf", b"fontbytes"),
        ]);
        let changed = change_font(&mut container, "Old", None).unwrap();
        assert!(changed);
        assert!(!container.has_name("old.otf"));
        let sheet = container.parsed_stylesheet("style.css").unwrap();
        assert_eq!(sheet.rules.len(), 0);
    }
}
