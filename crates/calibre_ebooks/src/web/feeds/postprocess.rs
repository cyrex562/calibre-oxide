//! Port of `BasicNewsRecipe`'s HTML cleanup/postprocessing pipeline
//! (issue #620, split from #81): `_postprocess_html` and its small
//! supporting hook methods. See `web::feeds`'s own module doc for the
//! full cluster context.
//!
//! # Real scope, narrower than first assumed
//!
//! `remove_tags`/`keep_only_tags`/`remove_tags_after`/
//! `remove_tags_before`/`preprocess_regexps` (issue #619's own
//! [`super::recipe::RecipeConfig`] fields) are **not** applied inside
//! `_postprocess_html` at all -- confirmed by reading `news.py`'s real
//! `download`/`__init__` methods, not assumed: they're merely copied
//! onto `self.web2disk_options` (`for extra in ('keep_only_tags',
//! 'remove_tags', 'preprocess_regexps', ...): setattr(...)`), for
//! `RecursiveFetcher` (issue #455, `web/fetch/simple.py`) to apply
//! during the actual fetch. `_postprocess_html` itself only ever does
//! the fixed, hardcoded cleanup this module ports -- stylesheet/
//! script/bad-tag removal, CSS injection, navbar insertion, and
//! HTML5-semantic-tag nuking. The tag-matching *engine* real
//! `remove_tags`/`keep_only_tags` need is #455/#623's job, not this
//! one's -- [`super::recipe::TagSpec`] already exists as pure data for
//! whenever that lands.
//!
//! # Also out of scope, disclosed why
//!
//! `populate_article_metadata` (real Python's last step, given a live
//! `Article` object from `self.feed_objects[f].articles[a]`) needs the
//! real per-article download-orchestration state issue #623 owns; this
//! module's [`postprocess_html`] is a pure `Dom -> Dom` transform with
//! no access to that state, so the hook isn't defined or called here.
//! #623 should call it directly once real `Feed`/`Article` state
//! exists alongside a processed `Dom`.

use calibre_utils::constants::APP_NAME;

use crate::dom::{Dom, NodeId, NodeKind};
use crate::web::feeds::recipe::{NewsRecipeHooks, RecipeConfig};
use crate::web::feeds::templates;

/// Port of `AbortArticle` (a plain exception type in real Python).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbortArticle(pub String);

/// The per-article context real `_postprocess_html` reads off
/// `job_info` to build the injected navbar. `None` (matching real
/// Python's `if first_fetch and job_info:` guard) skips navbar
/// insertion entirely -- used for pages fetched via `recursions`
/// (deeper link-following) that aren't "the" article page.
#[derive(Debug, Clone, Copy)]
pub struct NavbarContext<'a> {
    pub url: &'a str,
    pub feed_index: usize,
    pub article_index: usize,
    pub feed_len: usize,
    pub has_single_feed: bool,
}

/// Extends #619's [`NewsRecipeHooks`] with the postprocessing-pipeline
/// hooks (issue #620).
pub trait NewsRecipePostprocessHooks: NewsRecipeHooks {
    /// Port of `skip_ad_pages`. `Some(html)` means "this was an ad
    /// page, use this HTML instead"; `None` (the real default) means
    /// "not an ad page."
    fn skip_ad_pages(&self, _dom: &Dom) -> Option<String> {
        None
    }

    /// Port of `abort_article`.
    fn abort_article(&self, msg: Option<&str>) -> AbortArticle {
        AbortArticle(msg.map(str::to_string).unwrap_or_else(|| "Article download aborted".to_string()))
    }

    /// Port of `preprocess_raw_html`.
    fn preprocess_raw_html(&self, raw_html: String, _url: &str) -> String {
        raw_html
    }

    /// Port of `preprocess_html`.
    fn preprocess_html(&self, dom: Dom) -> Dom {
        dom
    }

    /// Port of `postprocess_html`.
    fn postprocess_html(&self, dom: Dom, _first_fetch: bool) -> Dom {
        dom
    }

    /// Port of `cleanup`.
    fn cleanup(&self) {}
}

/// Port of `preprocess_raw_html_`. **Disclosed**: real Python wraps
/// the `auto_cleanup` call in `try/except`, logging and falling back
/// to the unprocessed HTML on failure; this port's
/// [`super::recipe::extract_readable_article`] has no fallible path
/// (see `readability::Document::summary`'s own doc: this port's
/// implementation always succeeds, unlike the real Python it mirrors),
/// so there is no failure case to catch.
pub fn preprocess_raw_html<H: NewsRecipePostprocessHooks>(hooks: &H, raw_html: String, url: &str) -> String {
    let raw_html = hooks.preprocess_raw_html(raw_html, url);
    if hooks.config().auto_cleanup {
        crate::web::feeds::recipe::extract_readable_article(raw_html.as_bytes(), Some(url), hooks.config().auto_cleanup_keep.as_deref())
    } else {
        raw_html
    }
}

const BAD_TAGS: &[&str] = &["base", "iframe", "canvas", "embed", "button", "command", "datalist", "video", "audio", "noscript", "link", "meta"];
const HTML5_TAGS_TO_NUKE: &[&str] = &["article", "aside", "header", "footer", "nav", "figcaption", "figure", "section"];

fn has_attr(dom: &Dom, id: NodeId, attr: &str) -> bool {
    dom.nodes[id].attrs.contains_key(attr)
}

fn remove_elements(dom: &mut Dom, ids: Vec<NodeId>) {
    for id in ids {
        dom.detach(id);
    }
}

/// Copies `source_id`'s subtree (from `source`) into `target`'s own
/// node arena as a new, detached subtree -- needed to graft a
/// separately-rendered navbar page's `<div>` into the article's own
/// `Dom`, since the two documents don't share a node arena.
fn import_subtree(target: &mut Dom, source: &Dom, source_id: NodeId) -> NodeId {
    let node = &source.nodes[source_id];
    let new_id = match &node.kind {
        NodeKind::Element(tag) => {
            let id = target.new_element(tag);
            for (k, v) in &node.attrs {
                target.node_mut(id).attrs.insert(k.clone(), v.clone());
            }
            id
        }
        NodeKind::Text(t) => target.new_text(t),
        NodeKind::Comment(_) | NodeKind::Document => target.new_text(""),
    };
    let children: Vec<NodeId> = node.children.iter().map(|&c| import_subtree(target, source, c)).collect();
    for c in children {
        target.append_child(new_id, c);
    }
    new_id
}

/// Port of `_postprocess_html`. `touchscreen` selects
/// `NavBarTemplate` vs `TouchscreenNavBarTemplate`, matching real
/// Python's `self.navbar = TouchscreenNavBarTemplate() if
/// self.touchscreen else NavBarTemplate()` (an output-profile-derived
/// flag, not a `RecipeConfig` field -- so it's a parameter here, not
/// config).
pub fn postprocess_html<H: NewsRecipePostprocessHooks>(hooks: &H, mut dom: Dom, first_fetch: bool, navbar: Option<NavbarContext>, touchscreen: bool) -> Dom {
    let cfg = hooks.config();

    if cfg.no_stylesheets {
        let link_ids: Vec<NodeId> = dom
            .find_all_tag_global("link")
            .into_iter()
            .filter(|&id| {
                let is_css = dom.nodes[id].attrs.get("type").map(|s| s.to_lowercase()).unwrap_or_else(|| "text/css".to_string()) == "text/css";
                let is_stylesheet_rel = match dom.nodes[id].attrs.get("rel") {
                    Some(rel) => rel.split_whitespace().any(|w| w.eq_ignore_ascii_case("stylesheet")),
                    None => true, // real default: rel defaults to ('stylesheet',)
                };
                is_css && is_stylesheet_rel
            })
            .collect();
        remove_elements(&mut dom, link_ids);
        let style_ids = dom.find_all_tag_global("style");
        remove_elements(&mut dom, style_ids);
    }

    // Find head, falling back to body, falling back to the first
    // element at all -- matching real `soup.find('head') or
    // soup.find('body') or soup.find(True)`.
    let head_id = dom
        .find_first_tag_global("head")
        .or_else(|| dom.find_first_tag_global("body"))
        .or_else(|| dom.preorder_elements(dom.root).into_iter().next());
    if let Some(head_id) = head_id {
        let css = format!("{}\n\n{}", cfg.template_css, hooks.get_extra_css().unwrap_or_default());
        let style_id = dom.new_element("style");
        dom.node_mut(style_id).attrs.insert("type".to_string(), "text/css".to_string());
        dom.node_mut(style_id).attrs.insert("title".to_string(), "override_css".to_string());
        let text_id = dom.new_text(&css);
        dom.append_child(style_id, text_id);
        dom.append_child(head_id, style_id);
    }

    if first_fetch {
        if let (Some(ctx), Some(body_id)) = (navbar, dom.find_first_tag_global("body")) {
            let extra_css = hooks.get_extra_css().unwrap_or_default();
            let navbar_html = if touchscreen {
                templates::generate_touchscreen_navbar(false, ctx.feed_index, ctx.article_index, ctx.feed_len, ctx.url, APP_NAME, "", Some(&extra_css), None)
            } else {
                templates::generate_navbar(false, ctx.feed_index, ctx.article_index, ctx.feed_len, !ctx.has_single_feed, ctx.url, APP_NAME, "", cfg.center_navbar, Some(&extra_css), None)
            };
            let navbar_dom = Dom::parse(&navbar_html);
            if let Some(div_id) = navbar_dom.find_first_tag_global("div") {
                let imported = import_subtree(&mut dom, &navbar_dom, div_id);
                dom.insert_child(body_id, 0, imported);
            }
        }
    }

    if cfg.remove_javascript {
        let script_ids = dom.find_all_tag_global("script");
        remove_elements(&mut dom, script_ids);
        for id in dom.preorder_elements(dom.root) {
            if has_attr(&dom, id, "onload") {
                dom.node_mut(id).attrs.shift_remove("onload");
            }
        }
    }

    for attr in &cfg.remove_attributes {
        for id in dom.preorder_elements(dom.root) {
            if has_attr(&dom, id, attr) {
                dom.node_mut(id).attrs.shift_remove(attr.as_str());
            }
        }
    }

    for tag in BAD_TAGS {
        let ids = dom.find_all_tag_global(tag);
        remove_elements(&mut dom, ids);
    }

    for id in dom.find_all_tag_global("img") {
        if has_attr(&dom, id, "srcset") {
            dom.node_mut(id).attrs.shift_remove("srcset");
        }
    }

    let mut dom = hooks.postprocess_html(dom, first_fetch);

    for tag in HTML5_TAGS_TO_NUKE {
        for id in dom.find_all_tag_global(tag) {
            let suffix = format!("calibre-nuked-tag-{tag}");
            let existing = dom.nodes[id].attrs.get("class").cloned();
            let new_class = match existing {
                Some(c) if !c.is_empty() => format!("{c} {suffix}"),
                _ => suffix,
            };
            dom.node_mut(id).attrs.insert("class".to_string(), new_class);
            dom.node_mut(id).kind = NodeKind::Element("div".to_string());
        }
    }

    dom
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestRecipe(RecipeConfig);
    impl NewsRecipeHooks for TestRecipe {
        fn config(&self) -> &RecipeConfig {
            &self.0
        }
    }
    impl NewsRecipePostprocessHooks for TestRecipe {}

    fn recipe(cfg: RecipeConfig) -> TestRecipe {
        TestRecipe(cfg)
    }

    #[test]
    fn no_stylesheets_removes_stylesheet_links_and_style_tags() {
        // Real Python's later, unconditional `BAD_TAGS` pass removes
        // every remaining `<link>` regardless of `no_stylesheets`
        // anyway (link tags can be used for preloading, per its own
        // comment) -- this test only exercises the `no_stylesheets`-
        // specific removal reaching the stylesheet link and the style
        // tag; see `bad_tags_are_removed_unconditionally` for the
        // separate, unconditional link/meta removal.
        let r = recipe(RecipeConfig { no_stylesheets: true, ..Default::default() });
        let dom = Dom::parse(r#"<html><head><link rel="stylesheet" href="a.css"><style>.original-marker{}</style></head><body></body></html>"#);
        let out = postprocess_html(&r, dom, false, None, false);
        // The one remaining `<style>` is the freshly-injected real
        // `override_css` block (`postprocess_html` always adds one) --
        // the *original* stylesheet content must be gone.
        let style_ids = out.find_all_tag_global("style");
        assert!(!style_ids.iter().any(|&id| out.text_content(id).contains("original-marker")));
        assert!(out.find_all_tag_global("link").is_empty());
    }

    #[test]
    fn injects_the_real_override_css_into_head() {
        let r = recipe(RecipeConfig::default());
        let dom = Dom::parse("<html><head></head><body></body></html>");
        let out = postprocess_html(&r, dom, false, None, false);
        let head = out.find_first_tag_global("head").unwrap();
        let style_ids = out.find_all_tag(head, "style");
        assert_eq!(style_ids.len(), 1);
        assert_eq!(out.nodes[style_ids[0]].attrs.get("title").map(String::as_str), Some("override_css"));
        assert!(out.text_content(style_ids[0]).contains("calibre_navbar")); // from template_css
    }

    #[test]
    fn remove_javascript_strips_scripts_and_onload_attrs() {
        let r = recipe(RecipeConfig { remove_javascript: true, ..Default::default() });
        let dom = Dom::parse(r#"<html><body><script>alert(1)</script><div onload="x()">hi</div></body></html>"#);
        let out = postprocess_html(&r, dom, false, None, false);
        assert!(out.find_all_tag_global("script").is_empty());
        let divs = out.find_all_tag_global("div");
        assert!(!divs.iter().any(|&id| has_attr(&out, id, "onload")));
    }

    #[test]
    fn remove_attributes_strips_the_configured_attributes_everywhere() {
        let r = recipe(RecipeConfig { remove_attributes: vec!["style".to_string()], ..Default::default() });
        let dom = Dom::parse(r#"<html><body><p style="color:red">a</p><span style="color:blue">b</span></body></html>"#);
        let out = postprocess_html(&r, dom, false, None, false);
        for id in out.preorder_elements(out.root) {
            assert!(!has_attr(&out, id, "style"));
        }
    }

    #[test]
    fn bad_tags_are_removed_unconditionally() {
        let r = recipe(RecipeConfig::default());
        let dom = Dom::parse("<html><body><iframe src=\"x\"></iframe><video></video><meta charset=\"utf-8\"><p>keep me</p></body></html>");
        let out = postprocess_html(&r, dom, false, None, false);
        assert!(out.find_all_tag_global("iframe").is_empty());
        assert!(out.find_all_tag_global("video").is_empty());
        assert!(out.find_all_tag_global("meta").is_empty());
        assert!(!out.find_all_tag_global("p").is_empty());
    }

    #[test]
    fn srcset_is_stripped_from_images() {
        let r = recipe(RecipeConfig::default());
        let dom = Dom::parse(r#"<html><body><img src="a.jpg" srcset="a.jpg 1x, b.jpg 2x"></body></html>"#);
        let out = postprocess_html(&r, dom, false, None, false);
        let imgs = out.find_all_tag_global("img");
        assert_eq!(imgs.len(), 1);
        assert!(!has_attr(&out, imgs[0], "srcset"));
        assert!(has_attr(&out, imgs[0], "src"));
    }

    #[test]
    fn html5_semantic_tags_are_nuked_to_divs_with_a_marker_class() {
        let r = recipe(RecipeConfig::default());
        let dom = Dom::parse(r#"<html><body><article class="existing">content</article><section>more</section></body></html>"#);
        let out = postprocess_html(&r, dom, false, None, false);
        assert!(out.find_all_tag_global("article").is_empty());
        assert!(out.find_all_tag_global("section").is_empty());
        let divs = out.find_all_tag_global("div");
        let classes: Vec<String> = divs.iter().filter_map(|&id| out.nodes[id].attrs.get("class").cloned()).collect();
        assert!(classes.iter().any(|c| c == "existing calibre-nuked-tag-article"));
        assert!(classes.iter().any(|c| c == "calibre-nuked-tag-section"));
    }

    #[test]
    fn injects_a_real_navbar_div_as_the_first_body_child_on_first_fetch() {
        let r = recipe(RecipeConfig::default());
        let dom = Dom::parse("<html><body><p>Article text</p></body></html>");
        let ctx = NavbarContext { url: "http://example.com/a", feed_index: 0, article_index: 0, feed_len: 3, has_single_feed: false };
        let out = postprocess_html(&r, dom, true, Some(ctx), false);
        let body = out.find_first_tag_global("body").unwrap();
        let first_child = out.nodes[body].children[0];
        assert_eq!(out.tag(first_child), Some("div"));
        assert!(out.text_content(first_child).contains("Main menu"));
    }

    #[test]
    fn does_not_inject_a_navbar_when_not_first_fetch() {
        let r = recipe(RecipeConfig::default());
        let dom = Dom::parse("<html><body><p>Article text</p></body></html>");
        let ctx = NavbarContext { url: "http://example.com/a", feed_index: 0, article_index: 0, feed_len: 3, has_single_feed: false };
        let out = postprocess_html(&r, dom, false, Some(ctx), false);
        let body = out.find_first_tag_global("body").unwrap();
        assert!(!out.text_content(body).contains("Main menu"));
    }

    #[test]
    fn preprocess_raw_html_runs_auto_cleanup_only_when_enabled() {
        let raw = "<html><body><p>Some real article text long enough to be kept by the scoring heuristics, repeated for length to pass the content threshold used by the algorithm.</p></body></html>".to_string();

        let r_off = recipe(RecipeConfig { auto_cleanup: false, ..Default::default() });
        let out_off = preprocess_raw_html(&r_off, raw.clone(), "http://x/");
        assert_eq!(out_off, raw);

        let r_on = recipe(RecipeConfig { auto_cleanup: true, ..Default::default() });
        let out_on = preprocess_raw_html(&r_on, raw.clone(), "http://x/");
        assert_ne!(out_on, raw);
        assert!(out_on.contains("<title>"));
    }

    #[test]
    fn abort_article_carries_the_real_default_message() {
        let r = recipe(RecipeConfig::default());
        assert_eq!(r.abort_article(None).0, "Article download aborted");
        assert_eq!(r.abort_article(Some("custom")).0, "custom");
    }
}
