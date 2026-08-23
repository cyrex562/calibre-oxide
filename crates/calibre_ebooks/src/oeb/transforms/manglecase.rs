//! Port of `old_src/src/calibre/ebooks/oeb/transforms/manglecase.py`.
//!
//! # Style resolution: reused `cascade::resolve_property`, built a
//! narrow sheet-collector around it
//!
//! Python builds a full [`crate::oeb::stylizer::Stylizer`] per document
//! (already-resolved, inherited styles for every element) and just reads
//! `style['text-transform']`/`style['font-variant']` off it. This port
//! can't use `Stylizer` here: it wraps `roxmltree::Node`, which is
//! read-only, and this file mutates the very tree it's reading styles
//! from (see the batch-level task notes).
//!
//! [`crate::oeb::polish::cascade::resolve_property`] is exactly the
//! right tool for the *inheritance* half of this (ancestor-chain walk
//! over a `style_map`, against [`crate::dom::Dom`] -- already
//! `pub`, no loosening needed, reused as-is). What it does *not* provide
//! outside `oeb::polish` is the *sheet collection* half
//! (`resolve_styles`/`iterrules`, which take a `polish::Container` that
//! doesn't exist in the plain-`OEBBook` world this batch operates in).
//! [`build_style_map`] is a narrow, local reimplementation of that half,
//! scoped to exactly what this file needs: parse every `<style>`
//! element in the document plus every element's `style="..."`
//! attribute (both via [`crate::css`], the real CSS engine from issue
//! #164), match selectors with [`crate::css::Select`], and record each
//! element's own declared `text-transform`/`font-variant` (inline
//! `style=` always wins, matching CSS's normal specificity ordering for
//! an author style attribute). External/manifest-linked stylesheets are
//! not consulted here -- narrower than
//! [`crate::oeb::polish::cascade::resolve_styles`], which does walk the
//! full manifest, but `<style>` + inline covers everything a document
//!'s *own* markup can carry, and is a defensible scope for a
//! two-property, presentation-only transform.

use std::collections::HashMap;

use crate::css::Stylesheet;
use crate::dom::{Dom, NodeId, NodeKind};
use crate::oeb::book::OEBBook;
use crate::oeb::constants::CSS_MIME;
use crate::oeb::polish::cascade::{resolve_property, PropertyValue};

const CASE_MANGLER_CSS: &str =
    ".calibre_lowercase {\n    font-variant: normal;\n    font-size: 0.65em;\n}\n";

const TEXT_TRANSFORMS: &[&str] = &["capitalize", "uppercase", "lowercase"];

/// Port of `CaseMangler`: apply `text-transform`/`font-variant:
/// small-caps` as real markup changes, for output formats that can't
/// render those CSS properties themselves.
pub struct CaseMangler;

impl CaseMangler {
    pub fn call(&self, oeb: &mut OEBBook) {
        self.mangle_spine(oeb);
    }

    fn mangle_spine(&self, oeb: &mut OEBBook) {
        let (id, href) = oeb.manifest.generate("manglecase", "manglecase.css");
        oeb.manifest.add(&id, &href, CSS_MIME);
        let _ = oeb.container.write(&href, CASE_MANGLER_CSS.as_bytes());

        let spine_hrefs: Vec<String> = oeb
            .spine
            .iter()
            .filter_map(|s| oeb.manifest.get_by_id(&s.idref).map(|i| i.href.clone()))
            .collect();
        for item_href in spine_hrefs {
            let Ok(raw) = oeb.container.read(&item_href) else {
                continue;
            };
            let html = String::from_utf8_lossy(&raw);
            let mut dom = Dom::parse(&html);

            if let Some(head) = dom.find_first_tag_global("head") {
                let rel = super::filenames::relhref(&item_href, &href);
                let link = dom.new_element("link");
                dom.node_mut(link)
                    .attrs
                    .insert("rel".to_string(), "stylesheet".to_string());
                dom.node_mut(link).attrs.insert("href".to_string(), rel);
                dom.node_mut(link)
                    .attrs
                    .insert("type".to_string(), CSS_MIME.to_string());
                dom.append_child(head, link);
            }

            let style_map = build_style_map(&dom);
            if let Some(body) = dom.find_first_tag_global("body") {
                self.mangle_elem(&mut dom, body, &style_map);
            }

            let rendered = dom.serialize(dom.root).into_bytes();
            let _ = oeb.container.write(&item_href, &rendered);
        }
    }

    fn text_transform(&self, transform: &str, text: &str) -> String {
        match transform {
            "capitalize" => calibre_utils::icu::title_case(text),
            "uppercase" => calibre_utils::icu::upper(text),
            "lowercase" => calibre_utils::icu::lower(text),
            _ => text.to_string(),
        }
    }

    /// Port of `CaseMangler.mangle_elem`. `elem`'s own resolved
    /// `text-transform`/`font-variant` (via [`resolve_property`], which
    /// already walks ancestors for inheritance) is applied to `elem`'s
    /// direct text-node children; element children are recursed into
    /// (each computes its own resolved style for its own text). This is
    /// the same net effect as Python's `.text`/`.tail` walk -- see the
    /// module docs' link for why a Dom without a separate tail concept
    /// doesn't need to distinguish the two cases.
    fn mangle_elem(
        &self,
        dom: &mut Dom,
        elem: NodeId,
        style_map: &HashMap<NodeId, HashMap<String, PropertyValue>>,
    ) {
        let transform = resolve_property(style_map, dom, elem, "text-transform")
            .map(|v| v.css_text.trim().to_lowercase());
        let variant = resolve_property(style_map, dom, elem, "font-variant")
            .map(|v| v.css_text.trim().to_lowercase());

        for child in dom.children(elem) {
            let is_text = matches!(dom.node(child).kind, NodeKind::Text(_));
            if is_text {
                if let NodeKind::Text(text) = dom.node(child).kind.clone() {
                    if text.is_empty() {
                        continue;
                    }
                    let mut new_text = text.clone();
                    if let Some(t) = transform.as_deref() {
                        if TEXT_TRANSFORMS.contains(&t) {
                            new_text = self.text_transform(t, &new_text);
                        }
                    }
                    if new_text != text {
                        dom.node_mut(child).kind = NodeKind::Text(new_text);
                    }
                    if variant.as_deref() == Some("small-caps") {
                        smallcaps_text_node(dom, child);
                    }
                }
            } else {
                self.mangle_elem(dom, child, style_map);
            }
        }
    }
}

/// Port of `CaseMangler.split_text`: split `text` into runs of
/// consecutive characters that agree on `char.is_uppercase()` (not
/// "runs of letters" -- a digit/punctuation run groups with whichever
/// case-boolean it shares, exactly matching Python's `char.isupper()`
/// toggle).
fn split_text(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    let mut results = vec![String::new()];
    let mut is_upper = chars[0].is_uppercase();
    for &ch in &chars {
        if ch.is_uppercase() == is_upper {
            results.last_mut().unwrap().push(ch);
        } else {
            is_upper = !is_upper;
            results.push(ch.to_string());
        }
    }
    results
}

/// Port of Python's `str.isupper()`: true iff there is at least one
/// cased character and every cased character is uppercase.
fn is_all_upper(s: &str) -> bool {
    let mut has_cased = false;
    for c in s.chars() {
        if c.is_lowercase() {
            return false;
        }
        if c.is_uppercase() {
            has_cased = true;
        }
    }
    has_cased
}

/// Port of `CaseMangler.smallcaps_elem`: replace a text node with a
/// sequence of siblings -- uppercase runs kept as plain text, other runs
/// wrapped in `<span class="calibre_lowercase">` (rendered small by
/// [`CASE_MANGLER_CSS`]) with their text uppercased, faking small-caps
/// rendering for formats that don't support `font-variant`.
fn smallcaps_text_node(dom: &mut Dom, text_node: NodeId) {
    let Some(parent) = dom.parent(text_node) else {
        return;
    };
    let Some(idx) = dom.index_in_parent(text_node) else {
        return;
    };
    let text = match &dom.node(text_node).kind {
        NodeKind::Text(t) => t.clone(),
        _ => return,
    };
    if text.is_empty() {
        return;
    }
    dom.detach(text_node);
    for (offset, run) in split_text(&text).into_iter().enumerate() {
        let insert_idx = idx + offset;
        if is_all_upper(&run) {
            let node = dom.new_text(&run);
            dom.insert_child(parent, insert_idx, node);
        } else {
            let span = dom.new_element("span");
            dom.node_mut(span)
                .attrs
                .insert("class".to_string(), "calibre_lowercase".to_string());
            let upper = dom.new_text(&calibre_utils::icu::upper(&run));
            dom.append_child(span, upper);
            dom.insert_child(parent, insert_idx, span);
        }
    }
}

/// Build a per-element "own declared properties" map (`text-transform`/
/// `font-variant` only) from a document's `<style>` elements and
/// `style="..."` attributes. See the module docs for scope.
fn build_style_map(dom: &Dom) -> HashMap<NodeId, HashMap<String, PropertyValue>> {
    let mut map: HashMap<NodeId, HashMap<String, PropertyValue>> = HashMap::new();
    let elements = crate::css::matcher::dom_elements(dom);
    for style_el in dom.find_all_tag_global("style") {
        let css_text = dom.text_content(style_el);
        let sheet = Stylesheet::parse(&css_text);
        for rule in sheet.style_rules() {
            let matched = crate::css::Select::new(elements.clone()).matching(&rule.selectors);
            for el in matched {
                for prop in ["text-transform", "font-variant"] {
                    let value = rule.style.get_property_value(prop);
                    if !value.is_empty() {
                        map.entry(el.id).or_default().insert(
                            prop.to_string(),
                            PropertyValue::new(value.to_string(), None, false),
                        );
                    }
                }
            }
        }
    }
    for el in dom.preorder_elements(dom.root) {
        let Some(style_attr) = dom.node(el).attrs.get("style").cloned() else {
            continue;
        };
        let block = crate::css::parser::parse_declaration_list(&style_attr);
        for prop in ["text-transform", "font-variant"] {
            let value = block.get_property_value(prop);
            if !value.is_empty() {
                map.entry(el).or_default().insert(
                    prop.to_string(),
                    PropertyValue::new(value.to_string(), None, false),
                );
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::transforms::test_support::Builder;

    #[test]
    fn uppercase_transform_rewrites_text_and_inherits_to_children() {
        let mut oeb = Builder::new()
            .page(
                "a.html",
                r#"<p style="text-transform: uppercase">shout <em>louder</em></p>"#,
            )
            .build();
        CaseMangler.call(&mut oeb);
        let raw = oeb.container.read("a.html").unwrap();
        let html = String::from_utf8_lossy(&raw);
        assert!(html.contains("SHOUT"), "{html}");
        assert!(html.contains("LOUDER"), "{html}");
        assert!(html.contains("manglecase.css"), "{html}");
    }

    #[test]
    fn small_caps_wraps_lowercase_runs_in_a_span() {
        let mut oeb = Builder::new()
            .page("a.html", r#"<p style="font-variant: small-caps">Hello</p>"#)
            .build();
        CaseMangler.call(&mut oeb);
        let raw = oeb.container.read("a.html").unwrap();
        let html = String::from_utf8_lossy(&raw);
        assert!(html.contains("calibre_lowercase"), "{html}");
        assert!(html.contains("ELLO"), "{html}");
    }

    #[test]
    fn capitalize_uses_title_case() {
        let mut oeb = Builder::new()
            .page(
                "a.html",
                r#"<p style="text-transform: capitalize">an old man</p>"#,
            )
            .build();
        CaseMangler.call(&mut oeb);
        let raw = oeb.container.read("a.html").unwrap();
        let html = String::from_utf8_lossy(&raw);
        assert!(html.contains("An Old Man"), "{html}");
    }
}
