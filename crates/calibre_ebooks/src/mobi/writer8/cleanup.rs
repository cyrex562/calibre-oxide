//! KF8-specific markup cleanups applied before chunking.
//!
//! Port of `calibre.ebooks.mobi.writer8.cleanup`.
//!
//! # Scope gap: `CSSCleanup`
//!
//! Python's `CSSCleanup.__call__` drops the computed `height` CSS
//! property from every `<body>` element ("The Kindle touch displays all
//! black pages if the height is set on body"). It does this through a
//! `Stylizer` -- an object that resolves the full CSS cascade (external
//! stylesheets + `<style>` + inline `style=`) down to a single computed
//! style per element, then lets the caller drop individual properties
//! from *that* view. Nothing in this crate implements a CSS cascade
//! resolver (this is the same `css_parser` gap `main.rs`'s
//! `extract_css_into_flows` documents), so there is no way to tell
//! whether a given `<body>` even has a `height` in its *computed* style
//! (as opposed to a literal inline `style="height:..."` attribute, which
//! covers only a narrow slice of the real cases -- e.g. a `body { height:
//! ... }` rule in a linked stylesheet would be invisible to a
//! text-substitution shortcut). Rather than silently mishandle the
//! stylesheet cases, this is left as a documented gap: [`css_cleanup`]
//! only strips a literal `height` declaration from `<body>`'s own inline
//! `style` attribute (real, narrow, and always correct as far as it
//! goes), and does not attempt cascade resolution.
//!
//! [`remove_duplicate_anchors`] has no such dependency (it is pure
//! attribute-table bookkeeping over the parsed tree) and is ported in
//! full.

use std::collections::HashSet;

use crate::mobi::dom::Dom;

/// Strip a literal `height` declaration from every `<body>` element's own
/// inline `style=` attribute. Narrow stand-in for `CSSCleanup` -- see the
/// module scope note for why full cascade-aware height-dropping isn't
/// implemented.
pub fn css_cleanup(dom: &mut Dom) {
    for body in dom.find_all_tag_global("body") {
        let Some(style) = dom.node(body).attrs.get("style").cloned() else {
            continue;
        };
        let kept: Vec<String> = style
            .split(';')
            .map(str::trim)
            .filter(|decl| {
                !decl
                    .split(':')
                    .next()
                    .map(|prop| prop.trim().eq_ignore_ascii_case("height"))
                    .unwrap_or(false)
            })
            .filter(|decl| !decl.is_empty())
            .map(str::to_string)
            .collect();
        if kept.is_empty() {
            dom.node_mut(body).attrs.shift_remove("style");
        } else {
            dom.node_mut(body)
                .attrs
                .insert("style".to_string(), kept.join("; "));
        }
    }
}

/// Remove duplicate `id`/`name` attributes across every element in `dom`
/// (the Kindle mishandles duplicate anchors -- see
/// <https://bugs.launchpad.net/calibre/+bug/1454199>). Port of
/// `remove_duplicate_anchors`; operates on a single already-parsed
/// document (the per-item loop over `oeb.spine` lives in the caller,
/// matching how every other per-item pass in this port is structured).
pub fn remove_duplicate_anchors(dom: &mut Dom) {
    let mut seen: HashSet<String> = HashSet::new();
    for el in dom.preorder_elements(dom.root) {
        for attr in ["id", "name"] {
            let Some(anchor) = dom.node(el).attrs.get(attr).cloned() else {
                continue;
            };
            if seen.contains(&anchor) {
                dom.node_mut(el).attrs.shift_remove(attr);
            } else {
                seen.insert(anchor);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_cleanup_strips_inline_height_but_keeps_other_declarations() {
        let mut dom =
            Dom::parse("<html><body style=\"height: 100%; color: red\"><p>hi</p></body></html>");
        css_cleanup(&mut dom);
        let body = dom.find_first_tag_global("body").unwrap();
        let style = dom.node(body).attrs.get("style").cloned().unwrap();
        assert!(!style.to_lowercase().contains("height"), "{style}");
        assert!(style.contains("color: red"), "{style}");
    }

    #[test]
    fn remove_duplicate_anchors_keeps_the_first_occurrence() {
        let mut dom = Dom::parse(
            "<html><body><p id=\"x\">a</p><span id=\"x\">b</span><a name=\"x\">c</a></body></html>",
        );
        remove_duplicate_anchors(&mut dom);
        let ids: Vec<_> = dom
            .preorder_elements(dom.root)
            .into_iter()
            .filter_map(|n| dom.node(n).attrs.get("id").cloned())
            .collect();
        assert_eq!(ids, vec!["x".to_string()]);
        let span = dom.find_first_tag_global("span").unwrap();
        assert!(!dom.node(span).attrs.contains_key("id"));
        let a = dom.find_first_tag_global("a").unwrap();
        assert!(!dom.node(a).attrs.contains_key("name"));
    }
}
