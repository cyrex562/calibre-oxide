//! Port of `old_src/src/calibre/ebooks/oeb/transforms/unsmarten.py`.
//!
//! The actual character-substitution table this needs
//! (`calibre.utils.unsmarten.unsmarten_text`) is already ported --
//! [`calibre_utils::unsmarten::unsmarten_text`] -- so this file is just
//! the "walk every text/tail node in every spine-document body, skipping
//! `<pre>`" traversal around it.

use calibre_utils::unsmarten::unsmarten_text;

use crate::mobi::dom::{Dom, NodeId, NodeKind};
use crate::oeb::book::OEBBook;
use crate::oeb::constants::OEB_DOCS;

/// Port of `UnsmartenPunctuation`: replace smart quotes/dashes/ellipses
/// with their plain-ASCII equivalents throughout every document body,
/// except inside `<pre>`.
pub struct UnsmartenPunctuation;

impl UnsmartenPunctuation {
    /// Narrower than Python by one edge case: lxml distinguishes an
    /// element's `.text` (before its first child) from its `.tail`
    /// (after its closing tag, but stored *on the element*, not its
    /// parent), so `if barename(x.tag) == 'pre': continue` skips a
    /// `<pre>` element's tail too -- text immediately following `</pre>`
    /// with no intervening element. This DOM has no separate tail
    /// concept (that text is just a `Text` sibling in the parent's
    /// child list), so this port unsmartens it. Harmless in the common
    /// case (there's rarely meaningful smart punctuation glued directly
    /// onto a `</pre>` close tag).
    fn unsmarten(&self, dom: &mut Dom, root: NodeId) {
        for el in dom.preorder_elements(root) {
            if dom.tag(el) == Some("pre") {
                continue;
            }
            for child in dom.children(el) {
                if let NodeKind::Text(text) = &dom.node(child).kind {
                    let new_text = unsmarten_text(text);
                    if &new_text != text {
                        dom.node_mut(child).kind = NodeKind::Text(new_text);
                    }
                }
            }
        }
    }

    pub fn call(&self, oeb: &mut OEBBook) {
        let items: Vec<(String, String)> = oeb
            .manifest
            .iter()
            .map(|i| (i.href.clone(), i.media_type.clone()))
            .collect();
        for (href, media_type) in items {
            if !OEB_DOCS.contains(&media_type.as_str()) {
                continue;
            }
            let Ok(raw) = oeb.container.read(&href) else {
                continue;
            };
            let html = String::from_utf8_lossy(&raw);
            let mut dom = Dom::parse(&html);
            let Some(body) = dom.find_first_tag_global("body") else {
                continue;
            };
            self.unsmarten(&mut dom, body);
            let rendered = dom.serialize(dom.root).into_bytes();
            let _ = oeb.container.write(&href, &rendered);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::transforms::test_support::Builder;

    #[test]
    fn smart_quotes_become_ascii_outside_pre() {
        let mut oeb = Builder::new()
            .page("a.html", "<p>\u{201c}Hello\u{201d}</p>")
            .build();
        UnsmartenPunctuation.call(&mut oeb);
        let raw = oeb.container.read("a.html").unwrap();
        let html = String::from_utf8_lossy(&raw);
        assert!(html.contains("\"Hello\""), "{html}");
        assert!(!html.contains('\u{201c}'), "{html}");
    }

    #[test]
    fn pre_content_is_left_alone() {
        let mut oeb = Builder::new()
            .page("a.html", "<pre>\u{201c}code\u{201d}</pre>")
            .build();
        UnsmartenPunctuation.call(&mut oeb);
        let raw = oeb.container.read("a.html").unwrap();
        let html = String::from_utf8_lossy(&raw);
        assert!(html.contains('\u{201c}'), "{html}");
    }
}
