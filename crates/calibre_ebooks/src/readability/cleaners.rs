//! Port of `old_src/src/calibre/ebooks/readability/cleaners.py`.
//!
//! `bad_attrs`/`htmlstrip`/[`clean_attributes`] and [`normalize_spaces`] are
//! ported directly. `html_cleaner` (an `lxml.html.clean.Cleaner` instance)
//! has no tree-sanitizer equivalent in this crate, so [`clean_html`] is a
//! fresh implementation of exactly the flag set the Python constructs it
//! with -- see that function's doc comment for the flag-by-flag mapping.

use lazy_static::lazy_static;
use regex::Regex;

use crate::mobi::dom::{Dom, NodeId, NodeKind};

lazy_static! {
    /// Port of `htmlstrip` in `cleaners.py`. The Python builds this from
    /// `bad_attrs = ['width', 'height', 'style', '[-a-z]*color',
    /// 'background[-a-z]*', 'on*']` joined with `|` inside a
    /// non-capturing group.
    ///
    /// Note the last alternative, `on*`: as a regex this means the
    /// literal character `o` followed by zero-or-more `n`s -- *not*
    /// "any attribute starting with `on`". A real event-handler
    /// attribute like `onclick="..."` does not match it (after `on`,
    /// the next literal char in the source is `c`, not a space or `=`,
    /// so the alternative fails and backtracking can't rescue it since
    /// there's no `.*` anywhere in the pattern). This looks like an
    /// upstream typo (`on.*` or `on\w*` was almost certainly intended),
    /// but it's what the shipped Python actually does, so it's ported
    /// byte-for-byte here rather than "fixed": an attribute literally
    /// named `on` or `onn`/`onnn`/... would be stripped by this regex;
    /// `onclick`/`onload`/etc. are not touched by `clean_attributes` (a
    /// separate, real defense against those -- attribute-name-prefix
    /// stripping, not this regex -- lives in [`super::html_cleaner`]'s
    /// `clean_html`, matching the Python's own two-layers-of-defense
    /// shape: `html_cleaner` runs before `clean_attributes` in every
    /// call site).
    static ref HTMLSTRIP: Regex = Regex::new(
        r#"(?i)<([^>]+) (?:width|height|style|[-a-z]*color|background[-a-z]*|on*) *= *(?:[^ "'>]+|'[^']+'|"[^"]+")([^>]*)>"#
    ).expect("static regex");
}

/// Port of `clean_attributes` in `cleaners.py`. Operates on serialized
/// HTML directly, matching the Python's string-based approach (rather
/// than a tree walk) so the "drop the *last* matching bad attribute,
/// re-scan, repeat" behavior of the original `while htmlstrip.search():
/// html = htmlstrip.sub(...)` loop is preserved exactly. Each `sub` call
/// still replaces *all* non-overlapping matches across the whole string
/// in one pass (like Python's `re.sub`), so this converges to the same
/// fully-clean fixed point the Python reaches -- it just may take
/// several iterations for a tag carrying more than one bad attribute,
/// exactly as the original does.
pub fn clean_attributes(html: &str) -> String {
    let mut html = html.to_string();
    while HTMLSTRIP.is_match(&html) {
        html = HTMLSTRIP.replace_all(&html, "<$1$2>").to_string();
    }
    html
}

/// Port of `normalize_spaces` in `cleaners.py`: collapse any run of
/// whitespace to a single space, trimming ends.
pub fn normalize_spaces(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Fresh implementation of the Python's module-level `html_cleaner`:
///
/// ```python
/// html_cleaner = Cleaner(scripts=True, javascript=True, comments=True,
///                   style=True, links=True, meta=False, add_nofollow=False,
///                   page_structure=False, processing_instructions=True, embedded=False,
///                   frames=False, forms=False, annoying_tags=False, remove_tags=None,
///                   remove_unknown_tags=False, safe_attrs_only=False)
/// ```
///
/// `Cleaner.clean_html` mutates the tree in place, which is what this
/// does too. Flag-by-flag, given what's set here:
///
/// - `scripts=True` / `javascript=True`: `<script>` elements are dropped
///   (subtree removal). `javascript=True` additionally strips `on*`
///   event-handler attributes (`onclick`, `onload`, ...) from every
///   element, and neutralizes any attribute whose value is a
///   `javascript:`-scheme URL (e.g. `href="javascript:..."`) by removing
///   that attribute.
/// - `style=True`: `<style>` elements are dropped, and the `style`
///   attribute is stripped from every remaining element.
/// - `links=True`: `<link>` elements (stylesheets, `rel=` links, ...)
///   are dropped.
/// - `comments=True`: HTML comment nodes are dropped.
/// - `processing_instructions=True`: processing instructions are
///   dropped. **This is a no-op in this port**: `crate::mobi::dom::Dom`'s
///   `html5ever`-backed parser (see `convert()` in `dom.rs`) already
///   collapses every `RcNodeData::ProcessingInstruction` into an empty
///   `NodeKind::Comment(String::new())` node *at parse time*, before this
///   function ever runs -- there is no PI-shaped node left to drop by
///   the time `clean_html` sees the tree. Dropping all `Comment` nodes
///   below (for `comments=True`) incidentally also removes these
///   already-emptied ex-PI placeholders, which is harmless either way.
/// - Everything else (`meta`, `add_nofollow`, `page_structure`,
///   `embedded`, `frames`, `forms`, `annoying_tags`, `remove_tags`,
///   `remove_unknown_tags`, `safe_attrs_only`) is `False`/`None`/unset in
///   the Python, i.e. explicitly *not* requested, so none of it is
///   implemented here: `<meta>`/forms/frames/`<embed>`/`<object>`/
///   `<blink>`/`<marquee>` and every attribute not covered above are
///   left untouched, and no attribute-name allowlist is applied.
pub fn clean_html(dom: &mut Dom) {
    // Drop <script>/<style>/<link> subtrees.
    for tag in ["script", "style", "link"] {
        for id in dom.find_all_tag_global(tag) {
            dom.detach(id);
        }
    }

    // Drop every comment node (also mops up the empty ex-PI placeholders
    // noted above).
    let comments: Vec<NodeId> = dom
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| matches!(n.kind, NodeKind::Comment(_)))
        .map(|(id, _)| id)
        .collect();
    for id in comments {
        dom.detach(id);
    }

    // Strip `style` attributes, `on*` event-handler attributes, and
    // `javascript:`-scheme attribute values from every remaining
    // element.
    let elements = dom.preorder_elements(dom.root);
    for id in elements {
        let node = dom.node_mut(id);
        node.attrs.retain(|k, v| {
            let key_lower = k.to_ascii_lowercase();
            if key_lower == "style" {
                return false;
            }
            if key_lower.starts_with("on") {
                return false;
            }
            if is_javascript_scheme(v) {
                return false;
            }
            true
        });
    }
}

fn is_javascript_scheme(value: &str) -> bool {
    value
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("javascript:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_attributes_strips_bad_attrs() {
        let html = r#"<div width="100" height="50" style="color:red" class="ok">hi</div>"#;
        let out = clean_attributes(html);
        assert!(!out.contains("width"), "{out}");
        assert!(!out.contains("height"), "{out}");
        assert!(!out.contains("style"), "{out}");
        assert!(out.contains(r#"class="ok""#), "{out}");
    }

    #[test]
    fn clean_attributes_strips_color_and_background_variants() {
        let html = r#"<p bgcolor="red" background-image="x.png">hi</p>"#;
        let out = clean_attributes(html);
        assert!(!out.contains("bgcolor"), "{out}");
        assert!(!out.contains("background-image"), "{out}");
    }

    #[test]
    fn clean_attributes_multiple_bad_attrs_same_tag_all_removed() {
        // Exercises the "drop the last match, re-scan, repeat" while-loop
        // behavior: a tag with several bad attributes still ends up
        // fully clean, just via multiple internal iterations.
        let html = r#"<div width="1" height="2" style="color:red" bgcolor="blue">x</div>"#;
        let out = clean_attributes(html);
        assert_eq!(out, "<div>x</div>");
    }

    #[test]
    fn clean_attributes_on_star_quirk_does_not_strip_onclick() {
        // Documents the preserved upstream `on*` regex quirk: literal
        // event-handler attributes are NOT touched by this regex (they
        // are handled separately by `clean_html`'s explicit `on`-prefix
        // check instead).
        let html = r#"<a onclick="alert(1)">x</a>"#;
        let out = clean_attributes(html);
        assert!(out.contains("onclick"), "{out}");
    }

    #[test]
    fn normalize_spaces_collapses_whitespace() {
        assert_eq!(normalize_spaces("  a   b\n\tc  "), "a b c");
        assert_eq!(normalize_spaces(""), "");
    }

    #[test]
    fn clean_html_drops_script_style_link_and_comments() {
        let mut dom = Dom::parse(
            "<html><head><style>.a{}</style><link rel=\"stylesheet\" href=\"x.css\"></head>\
             <body><!-- hi --><script>alert(1)</script><p onclick=\"x()\" style=\"color:red\" \
             href=\"javascript:evil()\">text</p></body></html>",
        );
        clean_html(&mut dom);
        let html = dom.find_first_tag_global("html").unwrap();
        let out = dom.serialize(html);
        assert!(!out.contains("<style"), "{out}");
        assert!(!out.contains("<link"), "{out}");
        assert!(!out.contains("<script"), "{out}");
        assert!(!out.contains("<!--"), "{out}");
        assert!(!out.contains("onclick"), "{out}");
        assert!(!out.contains("javascript:"), "{out}");
        assert!(out.contains("text"), "{out}");
    }
}
