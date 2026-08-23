//! Port of `old_src/src/calibre/ebooks/readability/` -- calibre's bundled
//! port of the classic arc90/`python-readability` content-extraction
//! algorithm (same lineage as Mozilla's Readability.js): given a raw,
//! cluttered web page, strip nav/ads/sidebars and score the remaining
//! block elements to find the "main article" content.
//!
//! | Python | Rust |
//! | --- | --- |
//! | `__init__.py` (empty upstream) | this module |
//! | `cleaners.py` | [`cleaners`] |
//! | `debug.py` | [`debug`] |
//! | `htmls.py` | [`htmls`] |
//! | `readability.py` | [`readability`] |
//!
//! `readability.py`'s `Document` class is the real content: the scoring
//! heuristics that decide which block is "the article" are ported
//! arithmetic-for-arithmetic in [`readability::Document`].
//!
//! The only caller of this package in `old_src` is
//! `calibre.web.feeds.news` (RSS/news recipe fetching) -- a large,
//! separate, still-unported subsystem. This port doesn't depend on it
//! existing.

pub mod cleaners;
pub mod debug;
pub mod htmls;
// `readability::readability` mirrors the Python's own
// `readability/readability.py` (a file with the same name as its
// containing package) -- kept 1:1 rather than renamed, for the direct
// correspondence the doc table above documents.
#[allow(clippy::module_inception)]
pub mod readability;

pub use readability::{Document, DocumentOptions, ReadabilityLog, Unparsable};

use crate::dom::{Dom, NodeId, NodeKind};

/// lxml's `Element.text` property: the text immediately following the
/// opening tag, up to (but not including) the first child *element* --
/// i.e. the leading `Text` node(s) among `id`'s children, concatenated,
/// stopping at the first `Element` child.
///
/// This differs from [`Dom::text_content`], which concatenates *every*
/// text descendant recursively. Several call sites in `readability.py`
/// (`get_article`'s `<p>` sibling check, `get_title`,
/// `transform_misused_divs_into_paragraphs`) specifically rely on the
/// shallower `.text` semantics, so this is factored out as its own
/// helper rather than approximated with `text_content`.
pub(crate) fn direct_text(dom: &Dom, id: NodeId) -> Option<String> {
    let mut out = String::new();
    let mut any = false;
    for child in dom.children(id) {
        match &dom.node(child).kind {
            NodeKind::Text(t) => {
                out.push_str(t);
                any = true;
            }
            _ => break,
        }
    }
    if any {
        Some(out)
    } else {
        None
    }
}

/// lxml's `Element.tail` property, read via [`Dom::next_sibling`]: the
/// text immediately following `id`'s closing tag, up to the next
/// sibling element. `None` if the next sibling isn't a `Text` node.
pub(crate) fn tail_text(dom: &Dom, id: NodeId) -> Option<String> {
    match dom.next_sibling(id) {
        Some(n) => match &dom.node(n).kind {
            NodeKind::Text(t) => Some(t.clone()),
            _ => None,
        },
        None => None,
    }
}

/// lxml's iteration protocol over an `Element` (`list(element)`,
/// `enumerate(element)`, `len(element)`): only `Element` children, in
/// document order. Text/comment nodes are not part of the sequence at
/// all in lxml's model -- they live in `.text`/`.tail` instead (see
/// [`direct_text`]/[`tail_text`]).
pub(crate) fn element_children(dom: &Dom, id: NodeId) -> Vec<NodeId> {
    dom.children(id)
        .into_iter()
        .filter(|&c| matches!(dom.node(c).kind, NodeKind::Element(_)))
        .collect()
}
