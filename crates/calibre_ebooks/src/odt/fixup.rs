//! Port of `Extract.fix_markup` and the methods it calls
//! (`filter_css`/`extract_css`/`epubify_markup`/`apply_list_starts`),
//! operating on [`crate::mobi::dom::Dom`] (the mutable HTML arena from
//! issue #33) instead of `lxml.etree`, since this crate has no `lxml`
//! equivalent and `dom.rs` already exists for exactly this kind of
//! post-conversion tree-walk-and-mutate pass.

use crate::mobi::dom::{Dom, NodeId};
use crate::odt::css::{self, CssRule};
use std::collections::HashMap;

pub struct FixupOutput {
    /// The fully fixed-up XHTML document.
    pub html: String,
    /// The CSS pulled out of the inline `<style>` block, to be written to
    /// `odfpy.css` alongside the document (port of `extract_css` writing
    /// `open('odfpy.css', 'wb')`).
    pub external_css: String,
}

/// Port of `Extract.fix_markup`: runs `filter_css`, `extract_css`,
/// `epubify_markup`, and `apply_list_starts`, in that order (the same
/// order `fix_markup` calls them in, which matters: `filter_css` mutates
/// the still-inline `<style>` text, `extract_css` then moves that
/// *filtered* text out into a `<link>`+external stylesheet and parses it
/// for `epubify_markup`'s `get_css_for_class` lookups).
pub fn fix_markup(xhtml: &str, list_starts: &HashMap<String, String>) -> FixupOutput {
    let mut dom = Dom::parse(xhtml);
    filter_css(&mut dom);
    let rules = extract_css(&mut dom);
    epubify_markup(&mut dom, &rules);
    apply_list_starts(&mut dom, list_starts);
    FixupOutput {
        html: dom.serialize(dom.root),
        external_css: css::serialize_rules(&rules),
    }
}

fn find_style_node(dom: &Dom) -> Option<NodeId> {
    dom.find_all_tag_global("style")
        .into_iter()
        .find(|&id| dom.node(id).attrs.get("type").map(String::as_str) == Some("text/css"))
}

fn set_text(dom: &mut Dom, id: NodeId, text: &str) {
    for child in dom.children(id) {
        dom.detach(child);
    }
    let t = dom.new_text(text);
    dom.append_child(id, t);
}

/// Port of `Extract.filter_css`: consolidates the inline stylesheet's
/// multi-class-selector rules (see [`crate::odt::css::do_filter_css`])
/// and adds the resulting synthetic class to every element that carried
/// one of the original classes.
pub fn filter_css(dom: &mut Dom) {
    let Some(style_node) = find_style_node(dom) else {
        return;
    };
    let css_text = dom.text_content(style_node);
    if css_text.trim().is_empty() {
        return;
    }
    let (filtered, sel_map) = css::do_filter_css(&css_text);
    set_text(dom, style_node, &filtered);
    if sel_map.is_empty() {
        return;
    }
    for el in dom.preorder_elements(dom.root) {
        let Some(class_val) = dom.node(el).attrs.get("class").cloned() else {
            continue;
        };
        let mut extra: Vec<&str> = Vec::new();
        for cls in class_val.split_whitespace() {
            if let Some(v) = sel_map.get(cls) {
                extra.extend(v.iter().map(String::as_str));
            }
        }
        if extra.is_empty() {
            continue;
        }
        let mut new_val = class_val;
        for e in extra {
            new_val.push(' ');
            new_val.push_str(e);
        }
        dom.node_mut(el).attrs.insert("class".to_string(), new_val);
    }
}

/// Port of `Extract.extract_css`: moves the (by now filtered) inline
/// `<style type="text/css">` block out of the document, replacing it with
/// a `<link rel="stylesheet" href="odfpy.css">`, and returns the parsed
/// rules for `epubify_markup` to query.
pub fn extract_css(dom: &mut Dom) -> Vec<CssRule> {
    let Some(style_node) = find_style_node(dom) else {
        return Vec::new();
    };
    let css_text = dom.text_content(style_node);
    dom.detach(style_node);
    if let Some(head) = dom.find_first_tag_global("head") {
        let link = dom.new_element("link");
        dom.node_mut(link)
            .attrs
            .insert("type".to_string(), "text/css".to_string());
        dom.node_mut(link)
            .attrs
            .insert("rel".to_string(), "stylesheet".to_string());
        dom.node_mut(link)
            .attrs
            .insert("href".to_string(), "odfpy.css".to_string());
        dom.append_child(head, link);
    }
    css::parse_rules(&css_text)
}

/// Port of `Extract.epubify_markup`.
pub fn epubify_markup(dom: &mut Dom, rules: &[CssRule]) {
    fix_empty_title(dom);
    fix_p_div(dom);
    fix_contained_images(dom);
    fix_anchored_images(dom, rules);
}

/// "Fix empty title tags" -- `for t in XPath('//h:title')(root): if not
/// t.text: t.text = ' '`.
fn fix_empty_title(dom: &mut Dom) {
    if let Some(title) = dom.find_first_tag_global("title") {
        if dom.text_content(title).is_empty() {
            set_text(dom, title, " ");
        }
    }
}

/// "Fix `<p><div>` constructs as the asinine epubchecker complains about
/// them" -- every `<div>` that is a direct child of a `<p>` causes that
/// `<p>` to be renamed to `<div>`.
///
/// In practice `Dom::parse` (`html5ever`, HTML5 tree-construction rules)
/// already implicitly closes an open `<p>` before a block element like
/// `<div>` -- unlike `lxml.etree`'s strict-XML parsing, which is what lets
/// this invalid nesting survive into Python's tree in the first place.
/// This is kept as a defensive, faithful port (any nesting that does
/// survive parsing still gets fixed), but it is expected to be a no-op
/// against this crate's HTML5-tag-soup-normalized trees.
fn fix_p_div(dom: &mut Dom) {
    let ps_with_div_child: Vec<NodeId> = dom
        .find_all_tag_global("p")
        .into_iter()
        .filter(|&p| dom.children(p).iter().any(|&c| dom.tag(c) == Some("div")))
        .collect();
    for p in ps_with_div_child {
        dom.set_tag(p, "div");
    }
}

/// "Remove the position:relative ... Remove display: block on an image
/// inside a div" -- for a `<div>` whose sole child is an `<img style="…">`.
fn fix_contained_images(dom: &mut Dom) {
    let divs = dom.find_all_tag_global("div");
    for div in divs {
        let children = dom.children(div);
        if children.len() != 1 {
            continue;
        }
        let img = children[0];
        if dom.tag(img) != Some("img") || !dom.node(img).attrs.contains_key("style") {
            continue;
        }
        let mut style = dom
            .node(div)
            .attrs
            .get("style")
            .cloned()
            .unwrap_or_default();
        let trimmed = style.trim_end();
        if !trimmed.is_empty() && !trimmed.ends_with(';') {
            style = format!("{trimmed};");
        } else {
            style = trimmed.to_string();
        }
        style.push_str("position:static");
        dom.node_mut(div).attrs.insert("style".to_string(), style);
        dom.node_mut(img).attrs.insert(
            "style".to_string(),
            "max-width: 100%; max-height: 100%".to_string(),
        );
    }
}

/// "Handle anchored images" -- `div1 > div2 > img`, where `div1`/`div2`
/// each have exactly one child, converting margin-auto-based centering
/// into an explicit `text-align` on `div1` (readable by both WebKit and
/// ADE-style renderers).
fn fix_anchored_images(dom: &mut Dom, rules: &[CssRule]) {
    let imgs = dom.find_all_tag_global("img");
    for img in imgs {
        let Some(div2) = dom.parent(img) else {
            continue;
        };
        if dom.tag(div2) != Some("div") {
            continue;
        }
        let Some(div1) = dom.parent(div2) else {
            continue;
        };
        if dom.tag(div1) != Some("div") {
            continue;
        }
        if dom.children(div1).len() != 1 || dom.children(div2).len() != 1 {
            continue;
        }

        let cls1 = dom
            .node(div1)
            .attrs
            .get("class")
            .cloned()
            .unwrap_or_default();
        let mut has_align = cls1
            .split_whitespace()
            .filter_map(|c| css::get_css_for_class(rules, c))
            .any(|r| r.decls.contains_key("text-align"));

        if !has_align {
            let cls2 = dom
                .node(div2)
                .attrs
                .get("class")
                .cloned()
                .unwrap_or_default();
            let mut ml: Option<String> = None;
            let mut mr: Option<String> = None;
            for c in cls2.split_whitespace() {
                if let Some(rule) = css::get_css_for_class(rules, c) {
                    if let Some(v) = rule.decls.get("margin-left") {
                        ml = Some(v.clone());
                    }
                    if let Some(v) = rule.decls.get("margin-right") {
                        mr = Some(v.clone());
                    }
                }
            }
            let ml_auto = ml.as_deref() == Some("auto");
            let mr_auto = mr.as_deref() == Some("auto");
            let aval = if ml_auto && mr_auto {
                Some("center")
            } else if ml_auto && !mr_auto {
                Some("right")
            } else if !ml_auto && mr_auto {
                Some("left")
            } else {
                None
            };
            if let Some(aval) = aval {
                let mut style = dom
                    .node(div1)
                    .attrs
                    .get("style")
                    .cloned()
                    .unwrap_or_default();
                let trimmed = style.trim_end();
                style = if !trimmed.is_empty() && !trimmed.ends_with(';') {
                    format!("{trimmed};")
                } else {
                    trimmed.to_string()
                };
                style.push_str(&format!("text-align:{aval}"));
                has_align = true;
                dom.node_mut(div1).attrs.insert("style".to_string(), style);
            }
        }

        if has_align {
            let existing = dom
                .node(div2)
                .attrs
                .get("style")
                .cloned()
                .unwrap_or_default();
            dom.node_mut(div2)
                .attrs
                .insert("style".to_string(), format!("display:inline;{existing}"));
        }
    }
}

/// Port of `Extract.apply_list_starts`: for every `<ol class="…">`,
/// applies a `start` attribute if any of its (space-separated) class
/// tokens has a declared non-default start value.
pub fn apply_list_starts(dom: &mut Dom, list_starts: &HashMap<String, String>) {
    if list_starts.is_empty() {
        return;
    }
    for ol in dom.find_all_tag_global("ol") {
        let Some(class_val) = dom.node(ol).attrs.get("class").cloned() else {
            continue;
        };
        for cls in class_val.split_whitespace() {
            let key = format!(".{cls}");
            if let Some(val) = list_starts.get(&key) {
                dom.node_mut(ol)
                    .attrs
                    .insert("start".to_string(), val.clone());
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixes_empty_title() {
        let mut dom = Dom::parse("<html><head><title></title></head><body></body></html>");
        fix_empty_title(&mut dom);
        let title = dom.find_first_tag_global("title").unwrap();
        assert_eq!(dom.text_content(title), " ");
    }

    #[test]
    fn renames_p_containing_div_to_div() {
        // Built directly at the Dom level (rather than via `Dom::parse` of
        // literal `<p><div>...`) because html5ever's HTML5 tree
        // construction rules already implicitly close a `<p>` before a
        // `<div>` start tag, so parsing that text would never actually
        // produce a `<div>` nested inside a `<p>` to begin with -- see
        // `fix_p_div`'s doc comment.
        let mut dom = Dom::parse("<html><body></body></html>");
        let body = dom.find_first_tag_global("body").unwrap();
        let p = dom.new_element("p");
        let div = dom.new_element("div");
        dom.append_child(p, div);
        dom.append_child(body, p);

        fix_p_div(&mut dom);

        let divs = dom.find_all_tag_global("div");
        // The outer element (formerly <p>) is now a <div>, containing the original inner <div>.
        assert_eq!(divs.len(), 2);
        assert!(dom.find_all_tag_global("p").is_empty());
    }

    #[test]
    fn fixes_single_image_div() {
        let mut dom =
            Dom::parse(r#"<html><body><div><img style="display: block;"/></div></body></html>"#);
        fix_contained_images(&mut dom);
        let div = dom.find_first_tag_global("div").unwrap();
        assert_eq!(
            dom.node(div).attrs.get("style").map(String::as_str),
            Some("position:static")
        );
        let img = dom.find_first_tag_global("img").unwrap();
        assert_eq!(
            dom.node(img).attrs.get("style").map(String::as_str),
            Some("max-width: 100%; max-height: 100%")
        );
    }

    #[test]
    fn centers_anchored_image_via_margin_auto() {
        let html = r#"<html><head><style type="text/css">.G-A { margin-left: auto; margin-right: auto; }</style></head><body><div class="G-Outer"><div class="G-A"><img/></div></div></body></html>"#;
        let mut dom = Dom::parse(html);
        let rules = css::parse_rules(".G-A { margin-left: auto; margin-right: auto; }");
        fix_anchored_images(&mut dom, &rules);
        let outer = dom
            .find_all_tag_global("div")
            .into_iter()
            .find(|&d| dom.node(d).attrs.get("class").map(String::as_str) == Some("G-Outer"))
            .unwrap();
        assert_eq!(
            dom.node(outer).attrs.get("style").map(String::as_str),
            Some("text-align:center")
        );
        let inner = dom
            .find_all_tag_global("div")
            .into_iter()
            .find(|&d| dom.node(d).attrs.get("class").map(String::as_str) == Some("G-A"))
            .unwrap();
        assert_eq!(
            dom.node(inner).attrs.get("style").map(String::as_str),
            Some("display:inline;")
        );
    }

    #[test]
    fn applies_declared_list_start() {
        let mut dom =
            Dom::parse(r#"<html><body><ol class="MyList_1"><li>a</li></ol></body></html>"#);
        let mut starts = HashMap::new();
        starts.insert(".MyList_1".to_string(), "5".to_string());
        apply_list_starts(&mut dom, &starts);
        let ol = dom.find_first_tag_global("ol").unwrap();
        assert_eq!(
            dom.node(ol).attrs.get("start").map(String::as_str),
            Some("5")
        );
    }

    #[test]
    fn extract_css_moves_style_to_link() {
        let html = r#"<html><head><style type="text/css">.P-A { color: red; }</style></head><body></body></html>"#;
        let mut dom = Dom::parse(html);
        let rules = extract_css(&mut dom);
        assert_eq!(rules.len(), 1);
        assert!(dom.find_first_tag_global("style").is_none());
        let link = dom.find_first_tag_global("link").unwrap();
        assert_eq!(
            dom.node(link).attrs.get("href").map(String::as_str),
            Some("odfpy.css")
        );
    }
}
