//! Port of `render_book.py`'s `virtualize_html`/`rewrite_links`/
//! `anchor_map` (issue #480, part of #427's tracking epic): rewrite
//! an HTML document's link-bearing attributes into the in-browser
//! reader's virtualized `link_uid`-prefixed scheme, and produce the
//! `link_to_map`/anchor-id data `process_exploded_book` merges into
//! `book_render_data` (`calibre-book-manifest.json`).
//!
//! # Scope: the generic mechanism, not real book-resource resolution
//!
//! Upstream's actual URL virtualization (`create_link_replacer`)
//! resolves a relative href against the *exploded book's own
//! container* (`container.href_to_name`/`present_names` -- which
//! resource names really exist in this particular book) before
//! deciding to encode it, mark it `missing:`, or leave it alone. That
//! container doesn't exist in this crate yet -- it's issue #481's
//! territory (the book-extraction/orchestration piece this module's
//! own `link_uid`-decoding logic and #481 will both need to agree
//! on). So, matching the same split #479 already made for
//! `fast_css_transform.cpp` (real transform mechanism now, real
//! virtualization-scheme *decisions* once the container exists):
//!
//! - [`rewrite_link_attributes`] is the generic, callback-driven
//!   attribute walker (`rewrite_links`'s real mechanism) -- real and
//!   usable today with any callback, including a stub one in tests.
//! - [`encode_url`]/[`decode_url`]/[`encode_component`]/
//!   [`decode_component`] are the real `link_uid|base64(name)#frag|`
//!   encoding scheme itself (`render_book.py`'s own `encode_url`/
//!   `decode_url`, ported verbatim), usable by whatever #481 supplies
//!   as `rewrite_link_attributes`'s callback.
//! - [`process_anchor_links`] is the real, fully self-contained
//!   `handle_link` post-processing pass: given a document whose
//!   `href`/`src` attributes have *already* been virtualized (however
//!   that happened), walks every `<a>`/`<area>` and reports each
//!   as an internal link (populating `link_to_map`), a `missing:`
//!   reference, or an external link (get `target="_blank"`/`rel`)
//!   -- this needs no container at all, since it only interprets
//!   already-produced `link_uid`/`missing:`-prefixed strings.
//! - [`disable_non_stylesheet_links`] (`transform_html`'s own
//!   separate "disable non-stylesheet link tags" pass) and
//!   [`anchor_map`] are fully self-contained too.
//!
//! # Simplified: HTML vs. SVG `<a>`/`href` unified
//!
//! Upstream needs two separate XPath queries (`h:a`/`h:area` vs.
//! `svg:a`, and a `XLINK('href')`-qualified attribute name) because
//! lxml's XML parser preserves real namespaces. [`Dom`] (this port's
//! foundation, `html5ever`-backed) already collapses a foreign
//! attribute's namespace prefix into its bare local name during
//! parsing -- confirmed empirically: `<image xlink:href="x">` parses
//! to an attribute literally named `"href"`, indistinguishable from a
//! plain HTML `href`, same for an SVG `<a>`'s own tag name being
//! plain `"a"` just like an HTML anchor. So this module's own
//! `<a>`/`<area>` walk doesn't need two separate passes the way
//! upstream's does -- one pass over every `<a>`/`<area>` element's
//! `href` attribute already covers both cases.
//!
//! # Not ported: `<object>`'s codebase-relative link attributes
//!
//! [`rewrite_link_attributes`] treats every element uniformly.
//! Upstream's `iterlinks` special-cases `<object>`: its
//! `classid`/`data`/`archive` attributes are resolved relative to the
//! object's own `codebase` attribute before the callback ever sees
//! them, and `archive` is itself a space-separated list of URLs, not
//! one. Real, but `<object>` is essentially unused in EPUB content
//! (a general-purpose HTML feature, not something ebook readers
//! target) -- disclosed rather than silently mishandled.

use std::collections::{HashMap, HashSet};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

use crate::dom::{Dom, NodeId};

/// `lxml.html.defs.link_attrs`, plus `render_book.py`'s own
/// additions (`poster`, `altimg`) -- `xlink:href` isn't listed
/// separately since [`Dom`] already collapses it to plain `href`, see
/// this module's own doc.
const LINK_ATTRS: &[&str] = &["action", "archive", "background", "cite", "classid", "codebase", "data", "href", "longdesc", "profile", "src", "usemap", "poster", "altimg"];

/// Port of `rewrite_links`'s real mechanism (as used by
/// `virtualize_html`, i.e. with `find_links_in_css=False`): walks
/// every element in the subtree rooted at `root`, and for every
/// attribute whose name is in [`LINK_ATTRS`], replaces its value with
/// `callback(value)` when that returns `Some`. See this module's own
/// doc for what upstream behavior (`<object>` codebase-relative
/// resolution) isn't ported.
pub fn rewrite_link_attributes(dom: &mut Dom, root: NodeId, mut callback: impl FnMut(&str) -> Option<String>) {
    for id in dom.preorder_elements(root) {
        let names: Vec<String> = dom.node(id).attrs.keys().filter(|k| LINK_ATTRS.contains(&k.as_str())).cloned().collect();
        for name in names {
            let Some(value) = dom.node(id).attrs.get(&name).cloned() else { continue };
            if let Some(new_value) = callback(&value) {
                dom.node_mut(id).attrs.insert(name, new_value);
            }
        }
    }
}

/// Port of `polyglot.binary.as_base64_unicode`, as used by
/// `encode_url`: standard (padded) base64.
pub fn encode_component(s: &str) -> String {
    STANDARD.encode(s.as_bytes())
}

/// Port of `polyglot.binary.from_base64_unicode`.
pub fn decode_component(s: &str) -> Option<String> {
    let bytes = STANDARD.decode(s).ok()?;
    String::from_utf8(bytes).ok()
}

/// Port of `render_book.py`'s `encode_url`: a resource name (and
/// optional fragment) as it appears inside a `link_uid|...|` virtual
/// href.
pub fn encode_url(name: &str, frag: &str) -> String {
    let mut out = encode_component(name);
    if !frag.is_empty() {
        out.push('#');
        out.push_str(frag);
    }
    out
}

/// Port of `render_book.py`'s `decode_url`: the inverse of
/// [`encode_url`]. Returns `None` if `x`'s name portion isn't valid
/// base64/UTF-8 (upstream would raise; a caller here gets a clean
/// `None` to handle instead, matching this crate's general
/// don't-panic-on-untrusted-input posture).
pub fn decode_url(x: &str) -> Option<(String, String)> {
    let (name_part, frag) = match x.split_once('#') {
        Some((n, f)) => (n, f),
        None => (x, ""),
    };
    let name = decode_component(name_part)?;
    Some((name, frag.to_string()))
}

/// One entry an internal link produces: which document/fragment a
/// link inside `referrer` points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkKind {
    /// A real internal link, `link_uid|base64(name)[#frag]|` --
    /// `name`/`frag` are already decoded.
    Internal { name: String, frag: String },
    /// A `missing:name` reference -- upstream's marker for a link
    /// that resolved to a name not present in the book.
    Missing { name: String },
    /// Any other non-empty href -- a real external link.
    External,
}

/// Port of `virtualize_html`'s/`transform_html`'s shared
/// `handle_link` closure: walks every `<a>`/`<area>` element in the
/// subtree rooted at `root` (already-virtualized `href` values, see
/// this module's own doc), and for each:
/// - a `link_uid|...|` value: sets `href` to `javascript:void(0)`,
///   records `(name, frag) -> referrer` into `link_to_map`, sets
///   `data-{link_uid}` to a small JSON blob upstream's own reader JS
///   expects (`{"name":...,"frag":...}`).
/// - a `missing:name` value: same `javascript:void(0)` + `data-`
///   treatment, with `{"name":...,"frag":"","missing":true}`.
/// - any other non-empty value: a real external link, gets
///   `target="_blank"` + `rel="noopener noreferrer"`.
/// - an empty/absent `href` on an element that had the attribute at
///   all: set to `javascript:void(0)` (upstream's own dead-link
///   fallback).
///
/// `referrer` is `name` in upstream's own signature -- the document
/// this call is processing, recorded as the referring document for
/// every internal link found.
pub fn process_anchor_links(dom: &mut Dom, root: NodeId, link_uid: &str, referrer: &str, link_to_map: &mut HashMap<String, HashMap<String, HashSet<String>>>) {
    for id in dom.preorder_elements(root) {
        let tag = dom.tag(id).unwrap_or("");
        if tag != "a" && tag != "area" {
            continue;
        }
        let had_attr = dom.node(id).attrs.contains_key("href");
        let href = dom.node(id).attrs.get("href").cloned().unwrap_or_default();

        if let Some(rest) = href.strip_prefix(link_uid) {
            dom.node_mut(id).attrs.insert("href".to_string(), "javascript:void(0)".to_string());
            // Upstream: `href.split('|')[1]` -- the bare link_uid
            // (no `|...|` suffix, a same-page fragmentless self-link)
            // has no second `|`-part and is silently left at just the
            // void; only a real `link_uid|...|` value carries data.
            let encoded = rest.strip_prefix('|').and_then(|s| s.split('|').next());
            if let Some(encoded) = encoded {
                if let Some((lname, lfrag)) = decode_url(encoded) {
                    link_to_map.entry(lname.clone()).or_default().entry(lfrag.clone()).or_default().insert(referrer.to_string());
                    let data = serde_json::json!({"name": lname, "frag": lfrag});
                    dom.node_mut(id).attrs.insert(format!("data-{link_uid}"), data.to_string());
                }
            }
        } else if !href.is_empty() {
            if let Some(name) = href.strip_prefix("missing:") {
                dom.node_mut(id).attrs.insert("href".to_string(), "javascript:void(0)".to_string());
                let data = serde_json::json!({"name": name, "frag": "", "missing": true});
                dom.node_mut(id).attrs.insert(format!("data-{link_uid}"), data.to_string());
            } else {
                dom.node_mut(id).attrs.insert("target".to_string(), "_blank".to_string());
                dom.node_mut(id).attrs.insert("rel".to_string(), "noopener noreferrer".to_string());
            }
        } else if had_attr {
            dom.node_mut(id).attrs.insert("href".to_string(), "javascript:void(0)".to_string());
        }
    }
}

/// Port of `transform_html`'s "disable non-stylesheet link tags"
/// pass: clears every attribute on a `<link href="...">` element
/// unless it's (explicitly or by the same defaults upstream uses) a
/// `text/css` `stylesheet` link. Upstream's own comment explains why:
/// the browser will never load these anyway, and leaving them in
/// place hangs the reader's own resource-load check waiting for a
/// response that will never come.
pub fn disable_non_stylesheet_links(dom: &mut Dom, root: NodeId) {
    for id in dom.preorder_elements(root) {
        if dom.tag(id) != Some("link") || !dom.node(id).attrs.contains_key("href") {
            continue;
        }
        let ltype = dom.node(id).attrs.get("type").map(|s| s.to_lowercase()).unwrap_or_else(|| "text/css".to_string());
        let rel = dom.node(id).attrs.get("rel").map(|s| s.to_lowercase()).unwrap_or_else(|| "stylesheet".to_string());
        if ltype != "text/css" || rel != "stylesheet" {
            dom.node_mut(id).attrs.clear();
        }
    }
}

/// Port of `anchor_map`: every distinct `id` (or, for an `<a>` with
/// no `id`, its `name` promoted to `id`, matching upstream) in the
/// subtree rooted at `root`, in document order, first occurrence only.
pub fn anchor_map(dom: &mut Dom, root: NodeId) -> Vec<String> {
    let mut ans = Vec::new();
    let mut seen = HashSet::new();
    for id in dom.preorder_elements(root) {
        let mut eid = dom.node(id).attrs.get("id").cloned();
        if eid.is_none() && dom.tag(id) == Some("a") {
            if let Some(name) = dom.node(id).attrs.get("name").cloned() {
                dom.node_mut(id).attrs.insert("id".to_string(), name.clone());
                eid = Some(name);
            }
        }
        if let Some(eid) = eid {
            if !eid.is_empty() && seen.insert(eid.clone()) {
                ans.push(eid);
            }
        }
    }
    ans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_url_round_trips_a_name_and_fragment() {
        let encoded = encode_url("chapter2.xhtml", "section1");
        let (name, frag) = decode_url(&encoded).unwrap();
        assert_eq!(name, "chapter2.xhtml");
        assert_eq!(frag, "section1");
    }

    #[test]
    fn encode_decode_url_round_trips_with_no_fragment() {
        let encoded = encode_url("chapter2.xhtml", "");
        assert!(!encoded.contains('#'));
        let (name, frag) = decode_url(&encoded).unwrap();
        assert_eq!(name, "chapter2.xhtml");
        assert_eq!(frag, "");
    }

    #[test]
    fn decode_url_rejects_invalid_base64() {
        assert!(decode_url("not valid base64!!!").is_none());
    }

    #[test]
    fn rewrite_link_attributes_rewrites_href_and_src_but_not_other_attrs() {
        let mut dom = Dom::parse(r#"<html><body><a href="a.html">x</a><img src="b.jpg" alt="not a link"><p class="href">not an attribute match</p></body></html>"#);
        let root = dom.root;
        rewrite_link_attributes(&mut dom, root, |v| Some(v.to_uppercase()));

        let a = dom.find_first_tag_global("a").unwrap();
        assert_eq!(dom.node(a).attrs.get("href").unwrap(), "A.HTML");
        let img = dom.find_first_tag_global("img").unwrap();
        assert_eq!(dom.node(img).attrs.get("src").unwrap(), "B.JPG");
        assert_eq!(dom.node(img).attrs.get("alt").unwrap(), "not a link", "alt is not a link attribute");
        let p = dom.find_first_tag_global("p").unwrap();
        assert_eq!(dom.node(p).attrs.get("class").unwrap(), "href", "an attribute named 'class' with value 'href' is not itself rewritten");
    }

    #[test]
    fn rewrite_link_attributes_leaves_the_value_alone_when_the_callback_declines() {
        let mut dom = Dom::parse(r#"<html><body><a href="a.html">x</a></body></html>"#);
        let root = dom.root;
        rewrite_link_attributes(&mut dom, root, |_| None);
        let a = dom.find_first_tag_global("a").unwrap();
        assert_eq!(dom.node(a).attrs.get("href").unwrap(), "a.html");
    }

    #[test]
    fn rewrite_link_attributes_also_rewrites_a_collapsed_xlink_href() {
        let mut dom = Dom::parse(r#"<html><body><svg><image xlink:href="pic.jpg"/></svg></body></html>"#);
        let root = dom.root;
        rewrite_link_attributes(&mut dom, root, |v| Some(v.to_uppercase()));
        let image = dom.find_first_tag_global("image").unwrap();
        assert_eq!(dom.node(image).attrs.get("href").unwrap(), "PIC.JPG");
    }

    fn virtualized_href(name: &str, frag: &str) -> String {
        format!("LINKUID|{}|", encode_url(name, frag))
    }

    #[test]
    fn process_anchor_links_records_an_internal_link_and_voids_the_href() {
        let href = virtualized_href("chapter2.xhtml", "section1");
        let mut dom = Dom::parse(&format!(r#"<html><body><a href="{href}">go</a></body></html>"#));
        let root = dom.root;
        let mut link_to_map = HashMap::new();
        process_anchor_links(&mut dom, root, "LINKUID", "chapter1.xhtml", &mut link_to_map);

        let a = dom.find_first_tag_global("a").unwrap();
        assert_eq!(dom.node(a).attrs.get("href").unwrap(), "javascript:void(0)");
        let data: serde_json::Value = serde_json::from_str(dom.node(a).attrs.get("data-LINKUID").unwrap()).unwrap();
        assert_eq!(data["name"], "chapter2.xhtml");
        assert_eq!(data["frag"], "section1");

        let referrers = &link_to_map["chapter2.xhtml"]["section1"];
        assert!(referrers.contains("chapter1.xhtml"));
    }

    #[test]
    fn process_anchor_links_handles_a_bare_self_link_with_no_data() {
        let mut dom = Dom::parse(r#"<html><body><a href="LINKUID">top</a></body></html>"#);
        let root = dom.root;
        let mut link_to_map = HashMap::new();
        process_anchor_links(&mut dom, root, "LINKUID", "chapter1.xhtml", &mut link_to_map);

        let a = dom.find_first_tag_global("a").unwrap();
        assert_eq!(dom.node(a).attrs.get("href").unwrap(), "javascript:void(0)");
        assert!(dom.node(a).attrs.get("data-LINKUID").is_none());
        assert!(link_to_map.is_empty());
    }

    #[test]
    fn process_anchor_links_marks_a_missing_reference() {
        let mut dom = Dom::parse(r#"<html><body><a href="missing:ghost.xhtml">go</a></body></html>"#);
        let root = dom.root;
        let mut link_to_map = HashMap::new();
        process_anchor_links(&mut dom, root, "LINKUID", "chapter1.xhtml", &mut link_to_map);

        let a = dom.find_first_tag_global("a").unwrap();
        assert_eq!(dom.node(a).attrs.get("href").unwrap(), "javascript:void(0)");
        let data: serde_json::Value = serde_json::from_str(dom.node(a).attrs.get("data-LINKUID").unwrap()).unwrap();
        assert_eq!(data["name"], "ghost.xhtml");
        assert_eq!(data["missing"], true);
    }

    #[test]
    fn process_anchor_links_marks_an_external_link() {
        let mut dom = Dom::parse(r#"<html><body><a href="https://example.com">go</a></body></html>"#);
        let root = dom.root;
        let mut link_to_map = HashMap::new();
        process_anchor_links(&mut dom, root, "LINKUID", "chapter1.xhtml", &mut link_to_map);

        let a = dom.find_first_tag_global("a").unwrap();
        assert_eq!(dom.node(a).attrs.get("href").unwrap(), "https://example.com", "an external link's href is left as-is");
        assert_eq!(dom.node(a).attrs.get("target").unwrap(), "_blank");
        assert_eq!(dom.node(a).attrs.get("rel").unwrap(), "noopener noreferrer");
    }

    #[test]
    fn process_anchor_links_voids_an_empty_href() {
        let mut dom = Dom::parse(r#"<html><body><a href="">go</a></body></html>"#);
        let root = dom.root;
        let mut link_to_map = HashMap::new();
        process_anchor_links(&mut dom, root, "LINKUID", "chapter1.xhtml", &mut link_to_map);

        let a = dom.find_first_tag_global("a").unwrap();
        assert_eq!(dom.node(a).attrs.get("href").unwrap(), "javascript:void(0)");
    }

    #[test]
    fn process_anchor_links_ignores_non_anchor_elements() {
        let mut dom = Dom::parse(r#"<html><body><img src="LINKUID|xyz|"></body></html>"#);
        let root = dom.root;
        let mut link_to_map = HashMap::new();
        process_anchor_links(&mut dom, root, "LINKUID", "chapter1.xhtml", &mut link_to_map);
        let img = dom.find_first_tag_global("img").unwrap();
        assert_eq!(dom.node(img).attrs.get("src").unwrap(), "LINKUID|xyz|", "process_anchor_links only touches <a>/<area>");
    }

    #[test]
    fn disable_non_stylesheet_links_leaves_a_plain_stylesheet_link_alone() {
        let mut dom = Dom::parse(r#"<html><head><link href="style.css"></head></html>"#);
        let root = dom.root;
        disable_non_stylesheet_links(&mut dom, root);
        let link = dom.find_first_tag_global("link").unwrap();
        assert_eq!(dom.node(link).attrs.get("href").unwrap(), "style.css");
    }

    #[test]
    fn disable_non_stylesheet_links_leaves_an_explicit_text_css_stylesheet_alone() {
        let mut dom = Dom::parse(r#"<html><head><link href="style.css" type="text/css" rel="stylesheet"></head></html>"#);
        let root = dom.root;
        disable_non_stylesheet_links(&mut dom, root);
        let link = dom.find_first_tag_global("link").unwrap();
        assert_eq!(dom.node(link).attrs.get("href").unwrap(), "style.css");
    }

    #[test]
    fn disable_non_stylesheet_links_clears_a_non_css_link() {
        let mut dom = Dom::parse(r#"<html><head><link href="icon.png" rel="icon" type="image/png"></head></html>"#);
        let root = dom.root;
        disable_non_stylesheet_links(&mut dom, root);
        let link = dom.find_first_tag_global("link").unwrap();
        assert!(dom.node(link).attrs.is_empty(), "attrs: {:?}", dom.node(link).attrs);
    }

    #[test]
    fn disable_non_stylesheet_links_clears_a_link_with_the_wrong_rel() {
        let mut dom = Dom::parse(r#"<html><head><link href="alt.css" rel="alternate stylesheet" type="text/css"></head></html>"#);
        let root = dom.root;
        disable_non_stylesheet_links(&mut dom, root);
        let link = dom.find_first_tag_global("link").unwrap();
        assert!(dom.node(link).attrs.is_empty());
    }

    #[test]
    fn disable_non_stylesheet_links_ignores_a_link_with_no_href() {
        let mut dom = Dom::parse(r#"<html><head><link rel="icon"></head></html>"#);
        let root = dom.root;
        disable_non_stylesheet_links(&mut dom, root);
        let link = dom.find_first_tag_global("link").unwrap();
        assert_eq!(dom.node(link).attrs.get("rel").unwrap(), "icon", "no href means transform_html's own xpath never selects it");
    }

    #[test]
    fn anchor_map_collects_ids_in_document_order() {
        let mut dom = Dom::parse(r#"<html><body><p id="one">a</p><p id="two">b</p></body></html>"#);
        let root = dom.root;
        assert_eq!(anchor_map(&mut dom, root), vec!["one".to_string(), "two".to_string()]);
    }

    #[test]
    fn anchor_map_promotes_an_anchors_name_to_id_when_it_has_no_id() {
        let mut dom = Dom::parse(r#"<html><body><a name="legacy-anchor">x</a></body></html>"#);
        let root = dom.root;
        let ids = anchor_map(&mut dom, root);
        assert_eq!(ids, vec!["legacy-anchor".to_string()]);
        let a = dom.find_first_tag_global("a").unwrap();
        assert_eq!(dom.node(a).attrs.get("id").unwrap(), "legacy-anchor", "the name should be promoted onto a real id attribute too");
    }

    #[test]
    fn anchor_map_does_not_duplicate_a_repeated_id() {
        let mut dom = Dom::parse(r#"<html><body><p id="dup">a</p><span id="dup">b</span></body></html>"#);
        let root = dom.root;
        assert_eq!(anchor_map(&mut dom, root), vec!["dup".to_string()]);
    }

    #[test]
    fn anchor_map_is_empty_for_a_document_with_no_ids_or_names() {
        let mut dom = Dom::parse(r#"<html><body><p>a</p></body></html>"#);
        let root = dom.root;
        assert!(anchor_map(&mut dom, root).is_empty());
    }
}
