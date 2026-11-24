//! Port of `RecursiveFetcher.get_soup` (`old_src/src/calibre/web/fetch/simple.py`,
//! issue #631, split from the #455 epic -- see `docs/modules_to_port.md`'s
//! `simple.py` entry for the full split rationale): raw HTML in, a
//! cleaned/filtered [`Dom`] out.
//!
//! Real order: `xml_to_unicode` -> `preprocess_raw_html` hook -> regex
//! massage (`preprocess_regexps` + comment stripping) -> parse ->
//! `skip_ad_pages` hook (may substitute the whole page and re-run the
//! steps above) -> `keep_only_tags` -> `remove_tags_after` ->
//! `remove_tags_before` -> `remove_tags` -> `preprocess_html` hook.
//! Every hook here (`skip_ad_pages`/`preprocess_raw_html`/
//! `preprocess_html`) is already real, from
//! [`crate::web::feeds::postprocess::NewsRecipePostprocessHooks`]
//! (issue #620) -- this module is purely the tag-matching engine and
//! the orchestration around those already-shipped pieces.
//!
//! # The tag-selector matching engine
//!
//! [`tag_matches`] implements real BeautifulSoup `find`/`findAll`
//! matching semantics against a [`TagSpec`] (issue #619, widened here
//! after grepping all 1077 real `.recipe` files' actual
//! `remove_tags`/`keep_only_tags` shapes -- not guessed): tag-name
//! matching (one name or a list), attribute matching (exact string or
//! membership in a list), a `class`-specific special case
//! (whitespace-token membership, matching BeautifulSoup's own
//! CSS-class handling -- by far the most common real shape, ~2000 of
//! ~2700 real `attrs=` uses), and `text=` (direct text-content
//! equality).
//!
//! **Disclosed, deliberate scope boundary**: real BeautifulSoup also
//! accepts a compiled regex or an arbitrary callable as an attrs
//! value (`attrs={'class': re.compile(...)}` / `attrs={'id': lambda x:
//! ...}`); real, but rare (a few dozen of ~1400 real uses) and not
//! representable in a static Rust data shape without a small
//! expression language of its own -- not worth building for this
//! port's own recipe-authoring needs. A `TagSpec` that needs this
//! falls outside what this matcher supports.

use std::sync::OnceLock;

use regex::Regex;

use crate::chardet::xml_to_unicode;
use crate::dom::{Dom, NodeId};
use crate::web::feeds::postprocess::{self, NewsRecipePostprocessHooks};
use crate::web::feeds::recipe::{TagAttrValue, TagSpec};

// ===================================================================
// Tag-selector matching
// ===================================================================

/// Port of BeautifulSoup's own tag-matching logic (`Tag._matches`/
/// `SoupStrainer.search_tag`), restricted to what [`TagSpec`]
/// represents. See this module's own doc for the deliberate scope
/// boundary (no regex/callable-valued attrs).
pub fn tag_matches(dom: &Dom, node: NodeId, spec: &TagSpec) -> bool {
    let Some(tag_name) = dom.tag(node) else {
        return false;
    };
    if let Some(names) = &spec.name {
        if !names.iter().any(|n| n == tag_name) {
            return false;
        }
    }
    for (attr_name, expected) in &spec.attrs {
        if !attr_matches(dom, node, attr_name, expected) {
            return false;
        }
    }
    if let Some(expected_text) = &spec.text {
        if &dom.text_content(node) != expected_text {
            return false;
        }
    }
    true
}

fn attr_matches(dom: &Dom, node: NodeId, attr_name: &str, expected: &TagAttrValue) -> bool {
    let Some(actual) = dom.node(node).attrs.get(attr_name) else {
        return false;
    };
    if attr_name == "class" {
        let tokens: Vec<&str> = actual.split_whitespace().collect();
        return match expected {
            TagAttrValue::Str(s) => tokens.contains(&s.as_str()),
            TagAttrValue::List(list) => list.iter().any(|s| tokens.contains(&s.as_str())),
        };
    }
    match expected {
        TagAttrValue::Str(s) => actual == s,
        TagAttrValue::List(list) => list.iter().any(|s| actual == s),
    }
}

/// Port of `soup.findAll(**spec)`. `include_self` mirrors the real
/// difference between searching from the document root (`soup.find`,
/// which can itself "match" nothing since it isn't a real tag) versus
/// `tag.findAll` (which searches descendants only, excluding `scope`
/// itself) -- pass `false` when `scope` is a real tag being searched
/// the way [`crate::dom::Dom::find_all_tag`] does.
pub fn find_all_matching(dom: &Dom, scope: NodeId, spec: &TagSpec, include_self: bool) -> Vec<NodeId> {
    dom.preorder_elements(scope).into_iter().filter(|&n| (include_self || n != scope) && tag_matches(dom, n, spec)).collect()
}

/// Port of `soup.find(**spec)`.
pub fn find_first_matching(dom: &Dom, scope: NodeId, spec: &TagSpec) -> Option<NodeId> {
    find_all_matching(dom, scope, spec, true).into_iter().next()
}

// ===================================================================
// keep_only_tags / remove_tags_after / remove_tags_before / remove_tags
// ===================================================================

fn replace_node(dom: &mut Dom, old: NodeId, new: NodeId) {
    let Some(parent) = dom.node(old).parent else {
        return;
    };
    let idx = dom.node(parent).children.iter().position(|&c| c == old).unwrap_or(0);
    dom.detach(old);
    dom.insert_child(parent, idx, new);
}

/// Port of `get_soup`'s `keep_only_tags` block: builds a fresh `<body>`
/// containing only the matched subtrees (searched and moved spec by
/// spec, in order -- matching real Python's own sequential
/// `soup.find('body').findAll(**spec)` / `body.insert(...)` loop,
/// where each spec searches whatever remains of the *original* body
/// after earlier specs already moved their matches out), then swaps
/// it in for the original `<body>`. A no-op if there's no `<body>` at
/// all (real Python's `except AttributeError: pass`) or `specs` is
/// empty.
fn apply_keep_only_tags(dom: &mut Dom, specs: &[TagSpec]) {
    if specs.is_empty() {
        return;
    }
    let Some(body) = dom.find_first_tag_global("body") else {
        return;
    };
    let new_body = dom.new_element("body");
    for spec in specs {
        for m in find_all_matching(dom, body, spec, false) {
            dom.append_child(new_body, m);
        }
    }
    replace_node(dom, body, new_body);
}

/// Port of `get_soup`'s `remove_beyond(tag, next)` helper: climbs from
/// `start` up to (not including) `<body>`, at each level extracting
/// every one of that ancestor's own subsequent (`forward`) or
/// preceding (`!forward`) siblings -- net effect, delete everything in
/// the document after (or before) `start` in document order, keeping
/// `start` itself and its own descendants.
fn remove_beyond(dom: &mut Dom, start: NodeId, forward: bool) {
    let mut current = Some(start);
    while let Some(tag) = current {
        if dom.tag(tag) == Some("body") {
            break;
        }
        loop {
            let sibling = if forward { dom.next_sibling(tag) } else { dom.prev_sibling(tag) };
            match sibling {
                Some(s) => dom.detach(s),
                None => break,
            }
        }
        current = dom.parent(tag);
    }
}

fn apply_remove_tags_after(dom: &mut Dom, specs: &[TagSpec]) {
    for spec in specs {
        if let Some(tag) = find_first_matching(dom, dom.root, spec) {
            remove_beyond(dom, tag, true);
        }
    }
}

fn apply_remove_tags_before(dom: &mut Dom, specs: &[TagSpec]) {
    for spec in specs {
        if let Some(tag) = find_first_matching(dom, dom.root, spec) {
            remove_beyond(dom, tag, false);
        }
    }
}

/// Port of `for kwds in self.remove_tags: for tag in
/// soup.findAll(**kwds): tag.extract()`.
fn apply_remove_tags(dom: &mut Dom, specs: &[TagSpec]) {
    for spec in specs {
        for m in find_all_matching(dom, dom.root, spec, true) {
            dom.detach(m);
        }
    }
}

// ===================================================================
// get_soup
// ===================================================================

fn comment_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)<!--.*?-->").expect("static pattern"))
}

fn clean_raw_html<H: NewsRecipePostprocessHooks>(hooks: &H, src: &[u8], url: &str, preprocess_regexps: &[(Regex, String)]) -> String {
    let (decoded, _encoding) = xml_to_unicode(src, true, false);
    let mut usrc = postprocess::preprocess_raw_html(hooks, decoded, url);
    for (pat, repl) in preprocess_regexps {
        usrc = pat.replace_all(&usrc, repl.as_str()).into_owned();
    }
    comment_regex().replace_all(&usrc, "").into_owned()
}

/// Port of `RecursiveFetcher.get_soup`. `preprocess_regexps` is
/// `RecipeConfig::preprocess_regexps`, pre-compiled by the caller
/// (kept out of `RecipeConfig` itself, which only stores the literal
/// pattern/replacement strings -- issue #619's own scope).
/// `keep_only_tags`/`remove_tags_after`/`remove_tags_before`/
/// `remove_tags` are read directly off `hooks.config()`.
pub fn get_soup<H: NewsRecipePostprocessHooks>(hooks: &H, src: &[u8], url: Option<&str>, preprocess_regexps: &[(Regex, String)]) -> Dom {
    let url = url.unwrap_or("");
    let cleaned = clean_raw_html(hooks, src, url, preprocess_regexps);
    let mut dom = Dom::parse(&cleaned);

    if let Some(replacement) = hooks.skip_ad_pages(&dom) {
        let cleaned = clean_raw_html(hooks, replacement.as_bytes(), url, preprocess_regexps);
        dom = Dom::parse(&cleaned);
    }

    let cfg = hooks.config();
    apply_keep_only_tags(&mut dom, &cfg.keep_only_tags);
    apply_remove_tags_after(&mut dom, &cfg.remove_tags_after);
    apply_remove_tags_before(&mut dom, &cfg.remove_tags_before);
    apply_remove_tags(&mut dom, &cfg.remove_tags);

    hooks.preprocess_html(dom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::feeds::recipe::{NewsRecipeHooks, RecipeConfig};
    use std::collections::HashMap;

    struct TestRecipe(RecipeConfig);
    impl NewsRecipeHooks for TestRecipe {
        fn config(&self) -> &RecipeConfig {
            &self.0
        }
    }
    impl NewsRecipePostprocessHooks for TestRecipe {}

    fn dom_of(html: &str) -> Dom {
        Dom::parse(html)
    }

    fn spec(name: Option<&[&str]>, attrs: &[(&str, TagAttrValue)]) -> TagSpec {
        TagSpec { name: name.map(|n| n.iter().map(|s| s.to_string()).collect()), attrs: attrs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(), text: None }
    }

    // ===============================================================
    // tag_matches
    // ===============================================================

    #[test]
    fn tag_matches_by_name() {
        let dom = dom_of("<html><body><div>x</div><p>y</p></body></html>");
        let div = dom.find_first_tag_global("div").unwrap();
        let p = dom.find_first_tag_global("p").unwrap();
        let s = spec(Some(&["div"]), &[]);
        assert!(tag_matches(&dom, div, &s));
        assert!(!tag_matches(&dom, p, &s));
    }

    #[test]
    fn tag_matches_any_of_several_names() {
        let dom = dom_of("<html><body><h1>a</h1><h2>b</h2><h3>c</h3></body></html>");
        let s = spec(Some(&["h1", "h2"]), &[]);
        assert!(tag_matches(&dom, dom.find_first_tag_global("h1").unwrap(), &s));
        assert!(tag_matches(&dom, dom.find_first_tag_global("h2").unwrap(), &s));
        assert!(!tag_matches(&dom, dom.find_first_tag_global("h3").unwrap(), &s));
    }

    #[test]
    fn tag_matches_class_by_whitespace_token_membership() {
        let dom = dom_of(r#"<html><body><div class="ad promo">x</div><div class="promo-header">y</div></body></html>"#);
        let s = spec(None, &[("class", TagAttrValue::Str("promo".to_string()))]);
        let divs = dom.find_all_tag_global("div");
        assert!(tag_matches(&dom, divs[0], &s), "exact class token 'promo' should match even with other classes present");
        assert!(!tag_matches(&dom, divs[1], &s), "'promo-header' is a different token, not a substring match");
    }

    #[test]
    fn tag_matches_class_list_matches_if_any_token_present() {
        let dom = dom_of(r#"<html><body><div class="foo bar">x</div><div class="baz">y</div></body></html>"#);
        let s = spec(None, &[("class", TagAttrValue::List(vec!["bar".to_string(), "qux".to_string()]))]);
        let divs = dom.find_all_tag_global("div");
        assert!(tag_matches(&dom, divs[0], &s));
        assert!(!tag_matches(&dom, divs[1], &s));
    }

    #[test]
    fn tag_matches_non_class_attr_by_exact_string() {
        let dom = dom_of(r#"<html><body><div id="content">x</div><div id="content-2">y</div></body></html>"#);
        let s = spec(None, &[("id", TagAttrValue::Str("content".to_string()))]);
        let divs = dom.find_all_tag_global("div");
        assert!(tag_matches(&dom, divs[0], &s));
        assert!(!tag_matches(&dom, divs[1], &s), "non-class attrs require an exact match, not a substring");
    }

    #[test]
    fn tag_matches_requires_the_attribute_to_be_present() {
        let dom = dom_of("<html><body><div>x</div></body></html>");
        let s = spec(None, &[("id", TagAttrValue::Str("content".to_string()))]);
        assert!(!tag_matches(&dom, dom.find_first_tag_global("div").unwrap(), &s));
    }

    #[test]
    fn tag_matches_by_text() {
        let dom = dom_of("<html><body><p>Hello</p><p>World</p></body></html>");
        let s = TagSpec { name: None, attrs: HashMap::new(), text: Some("Hello".to_string()) };
        let ps = dom.find_all_tag_global("p");
        assert!(tag_matches(&dom, ps[0], &s));
        assert!(!tag_matches(&dom, ps[1], &s));
    }

    // ===============================================================
    // apply_remove_tags / find_all_matching
    // ===============================================================

    #[test]
    fn remove_tags_extracts_every_match() {
        let mut dom = dom_of(r#"<html><body><div class="ad">1</div><p>keep</p><div class="ad">2</div></body></html>"#);
        let specs = vec![spec(Some(&["div"]), &[("class", TagAttrValue::Str("ad".to_string()))])];
        apply_remove_tags(&mut dom, &specs);
        assert!(dom.find_all_tag_global("div").is_empty());
        assert_eq!(dom.find_all_tag_global("p").len(), 1);
    }

    // ===============================================================
    // keep_only_tags
    // ===============================================================

    #[test]
    fn keep_only_tags_rebuilds_the_body_from_matches_in_order() {
        let mut dom = dom_of(r#"<html><body><div id="header">H</div><div id="article">A</div><div id="footer">F</div></body></html>"#);
        let specs = vec![spec(None, &[("id", TagAttrValue::Str("article".to_string()))]), spec(None, &[("id", TagAttrValue::Str("header".to_string()))])];
        apply_keep_only_tags(&mut dom, &specs);

        let body = dom.find_first_tag_global("body").unwrap();
        let kept: Vec<&str> = dom.node(body).children.iter().filter_map(|&c| dom.tag(c)).collect();
        assert_eq!(kept, vec!["div", "div"], "only the two matched divs remain");
        // Order follows the spec list order (article's spec listed first).
        let ids: Vec<String> = dom.node(body).children.iter().map(|&c| dom.node(c).attrs.get("id").cloned().unwrap_or_default()).collect();
        assert_eq!(ids, vec!["article", "header"]);
    }

    #[test]
    fn keep_only_tags_is_a_noop_with_no_specs() {
        let mut dom = dom_of("<html><body><div>x</div></body></html>");
        apply_keep_only_tags(&mut dom, &[]);
        assert_eq!(dom.find_all_tag_global("div").len(), 1);
    }

    #[test]
    fn keep_only_tags_is_a_noop_with_no_body() {
        let mut dom = Dom::empty();
        let specs = vec![spec(Some(&["div"]), &[])];
        // Must not panic even though there's no <body> at all.
        apply_keep_only_tags(&mut dom, &specs);
    }

    // ===============================================================
    // remove_tags_after / remove_tags_before
    // ===============================================================

    #[test]
    fn remove_tags_after_deletes_everything_following_in_document_order() {
        let mut dom = dom_of(r#"<html><body><div id="a">A</div><div id="marker">M</div><div id="b">B</div><div id="c">C</div></body></html>"#);
        let specs = vec![spec(None, &[("id", TagAttrValue::Str("marker".to_string()))])];
        apply_remove_tags_after(&mut dom, &specs);
        let remaining: Vec<String> = dom.find_all_tag_global("div").iter().map(|&d| dom.node(d).attrs.get("id").cloned().unwrap_or_default()).collect();
        assert_eq!(remaining, vec!["a", "marker"], "marker itself and everything before it survive; everything after is removed");
    }

    #[test]
    fn remove_tags_before_deletes_everything_preceding_in_document_order() {
        let mut dom = dom_of(r#"<html><body><div id="a">A</div><div id="marker">M</div><div id="b">B</div></body></html>"#);
        let specs = vec![spec(None, &[("id", TagAttrValue::Str("marker".to_string()))])];
        apply_remove_tags_before(&mut dom, &specs);
        let remaining: Vec<String> = dom.find_all_tag_global("div").iter().map(|&d| dom.node(d).attrs.get("id").cloned().unwrap_or_default()).collect();
        assert_eq!(remaining, vec!["marker", "b"]);
    }

    #[test]
    fn remove_tags_after_climbs_through_nested_ancestors() {
        let mut dom = dom_of(r#"<html><body><section><div id="marker">M</div><div id="sibling">S</div></section><div id="after-section">X</div></body></html>"#);
        let specs = vec![spec(None, &[("id", TagAttrValue::Str("marker".to_string()))])];
        apply_remove_tags_after(&mut dom, &specs);
        let remaining: Vec<String> = dom.find_all_tag_global("div").iter().map(|&d| dom.node(d).attrs.get("id").cloned().unwrap_or_default()).collect();
        assert_eq!(remaining, vec!["marker"], "the marker's own sibling AND the section's own following sibling must both be removed");
    }

    #[test]
    fn remove_tags_after_is_a_noop_when_nothing_matches() {
        let mut dom = dom_of("<html><body><div>a</div><div>b</div></body></html>");
        let specs = vec![spec(Some(&["span"]), &[])];
        apply_remove_tags_after(&mut dom, &specs);
        assert_eq!(dom.find_all_tag_global("div").len(), 2);
    }

    // ===============================================================
    // get_soup: end-to-end
    // ===============================================================

    #[test]
    fn get_soup_applies_the_full_pipeline_in_order() {
        let mut cfg = RecipeConfig::default();
        cfg.keep_only_tags = vec![spec(None, &[("id", TagAttrValue::Str("article".to_string()))])];
        cfg.remove_tags = vec![spec(Some(&["script"]), &[])];
        let hooks = TestRecipe(cfg);

        let html = br#"<html><body>
            <script>evil()</script>
            <div id="nav">nav</div>
            <div id="article"><p>Real content</p><script>tracker()</script></div>
        </body></html>"#;

        let dom = get_soup(&hooks, html, Some("http://example.com/"), &[]);
        assert!(dom.find_all_tag_global("script").is_empty(), "remove_tags should strip scripts even inside the kept article");
        let body = dom.find_first_tag_global("body").unwrap();
        assert_eq!(dom.node(body).children.len(), 1, "keep_only_tags should have rebuilt the body with only the article");
        assert!(dom.text_content(body).contains("Real content"));
        assert!(!dom.text_content(body).contains("nav"));
    }

    #[test]
    fn get_soup_strips_html_comments() {
        let hooks = TestRecipe(RecipeConfig::default());
        let html = b"<html><body><!-- a comment --><p>Text</p></body></html>";
        let dom = get_soup(&hooks, html, None, &[]);
        assert!(!dom.serialize(dom.root).contains("a comment"));
        assert!(dom.text_content(dom.root).contains("Text"));
    }

    #[test]
    fn get_soup_applies_preprocess_regexps() {
        let hooks = TestRecipe(RecipeConfig::default());
        let html = b"<html><body><p>foo BAD_WORD bar</p></body></html>";
        let regexps = vec![(Regex::new("BAD_WORD").unwrap(), "***".to_string())];
        let dom = get_soup(&hooks, html, None, &regexps);
        assert!(dom.text_content(dom.root).contains("***"));
        assert!(!dom.text_content(dom.root).contains("BAD_WORD"));
    }

    #[test]
    fn get_soup_honors_skip_ad_pages_by_reparsing_the_replacement() {
        struct AdSkippingRecipe(RecipeConfig);
        impl NewsRecipeHooks for AdSkippingRecipe {
            fn config(&self) -> &RecipeConfig {
                &self.0
            }
        }
        impl NewsRecipePostprocessHooks for AdSkippingRecipe {
            fn skip_ad_pages(&self, dom: &Dom) -> Option<String> {
                if dom.text_content(dom.root).contains("Please wait") {
                    Some("<html><body><p>Real article</p></body></html>".to_string())
                } else {
                    None
                }
            }
        }
        let hooks = AdSkippingRecipe(RecipeConfig::default());
        let html = b"<html><body><p>Please wait, redirecting...</p></body></html>";
        let dom = get_soup(&hooks, html, None, &[]);
        assert!(dom.text_content(dom.root).contains("Real article"));
        assert!(!dom.text_content(dom.root).contains("Please wait"));
    }
}
