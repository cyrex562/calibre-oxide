//! Port of `old_src/src/calibre/ebooks/docx/index.py` -- **the
//! `polish_index_markup` half only** (issue #293): the recursive
//! block-merge algorithm that turns raw `XE`-field-generated index
//! entries into a properly nested index, once they've already become
//! real HTML ([`get_applicable_xe_fields`], [`split_up_block`],
//! [`find_match`], [`add_link`], [`merge_blocks`],
//! [`polish_index_markup`]).
//!
//! `make_block`, `add_xe`, and `process_index` are a separate,
//! larger follow-up: they insert synthetic `w:p`/`w:pPr`/`w:pStyle`/
//! `w:r`/`w:t`/`w:br` elements into the *source* document tree, the
//! exact same open architectural question `fields.rs`'s module docs
//! raise for `parse_xe`'s synthetic bookmark (`crate::xmltree` vs. a
//! tracked side-table `to_html.rs` can consult) -- worth resolving
//! once, for both files, rather than separately.
//!
//! # A reproduced upstream bug
//!
//! [`split_up_block`]'s final `ldict` entry uses the wrong value.
//! Python's
//! ```python
//! for i, prefix in enumerate(prefix):
//!     ...
//! span.append(a)
//! ldict[span] = len(prefix)
//! ```
//! shadows the outer `prefix` *list* with each individual *string*
//! element as the loop variable of the same name. After the loop,
//! `prefix` refers to the last string processed, not the list -- so
//! `len(prefix)` is that string's *character count*, not the intended
//! nesting depth (`len(prefix_list)`). For a two-part entry (or any
//! entry whose second-to-last segment happens to be exactly one
//! character) this is invisible, since both interpretations agree;
//! for anything longer it silently assigns the wrong merge-tree depth
//! to that span. Ported as-is.

use std::collections::HashMap;

use roxmltree::Node;

use super::block_styles::format_g3;
use super::names::DocxNamespace;
use crate::dom::{Dom, NodeId, NodeKind};

/// The four `parse_index` result keys `get_applicable_xe_fields`
/// actually reads (of the many `INDEX_FIELDS` flags `fields::parse_index`
/// recognizes -- the rest are parsed but never consulted anywhere in
/// this file, matching Python's own dead-but-parsed fields).
#[derive(Debug, Clone, Default)]
pub struct IndexField {
    pub heading: Option<String>,
    pub entry_type: Option<String>,
    pub letter_range: Option<String>,
    pub bookmark: Option<String>,
}

/// The `parse_xe` result fields `index.py` actually reads, plus the
/// two `Fields.parse_xe` (not yet ported) adds after calling
/// `fields::parse_xe`: `anchor` (the synthetic bookmark name) and
/// `start_elem` (the field's own `w:fldChar` begin element, used here
/// to check which bookmark, if any, contains this entry).
#[derive(Debug, Clone)]
pub struct XeField<'a, 'i> {
    pub text: String,
    pub entry_type: Option<String>,
    pub page_number_text: Option<String>,
    pub anchor: String,
    pub start_elem: Node<'a, 'i>,
}

/// Filters `xe_fields` down to the ones `index`'s own `entry-type`/
/// `letter-range`/`bookmark` restrict it to.
///
/// The `bookmark` filter's Python XPath (`XPath('//w:bookmarkStart')`,
/// an *absolute* path -- `//` always searches the whole document
/// regardless of context node) really means "every `w:bookmarkStart`
/// in the document named `bmark`", found from `xe_fields[0]`'s owning
/// document. Python indexes `xe_fields[0]` unconditionally here, which
/// would raise `IndexError` if `xe_fields` had already been filtered
/// down to nothing by the earlier `entry-type`/`letter-range` passes;
/// this port returns early with the (empty) list instead of
/// replicating that crash, since there is nothing useful to reproduce
/// about it.
///
/// Port of `get_applicable_xe_fields`.
pub fn get_applicable_xe_fields<'a, 'i>(
    index: &IndexField,
    xe_fields: Vec<XeField<'a, 'i>>,
    ns: &DocxNamespace,
) -> Vec<XeField<'a, 'i>> {
    let iet = index.entry_type.as_deref();
    let mut xe_fields: Vec<_> = xe_fields
        .into_iter()
        .filter(|xe| xe.entry_type.as_deref() == iet)
        .collect();

    if let Some(lr) = &index.letter_range {
        if let Some((sl, el)) = lr.split_once('-') {
            let sl = sl.trim();
            let el = el.trim();
            if !sl.is_empty() && !el.is_empty() {
                xe_fields.retain(|xe| {
                    let first = xe
                        .text
                        .chars()
                        .next()
                        .map(|c| c.to_string())
                        .unwrap_or_default();
                    sl <= first.as_str() && first.as_str() <= el
                });
            }
        }
    }

    let Some(bmark) = &index.bookmark else {
        return xe_fields;
    };
    if xe_fields.is_empty() {
        return xe_fields;
    }

    let root = xe_fields[0].start_elem.document().root_element();
    let bookmarks: std::collections::HashSet<Node> = ns
        .descendants(root, &["w:bookmarkStart"])
        .into_iter()
        .filter(|&b| ns.get(b, "w:name") == Some(bmark.as_str()))
        .collect();

    xe_fields
        .into_iter()
        .filter(|xe| {
            xe.start_elem
                .ancestors()
                .any(|a| ns.is_tag(a, "w:bookmarkStart") && bookmarks.contains(&a))
        })
        .collect()
}

/// lxml's `elem.text = text`: replaces the leading text (before the
/// first child element), leaving any child elements untouched.
fn set_leading_text(dom: &mut Dom, elem: NodeId, text: &str) {
    let children = dom.children(elem);
    if let Some(&first) = children.first() {
        if matches!(dom.node(first).kind, NodeKind::Text(_)) {
            dom.node_mut(first).kind = NodeKind::Text(text.to_string());
            return;
        }
    }
    let t = dom.new_text(text);
    dom.insert_child(elem, 0, t);
}

/// lxml's `elem.tail = text`: sets the text immediately following
/// `elem`, before its next sibling.
fn set_tail(dom: &mut Dom, elem: NodeId, text: &str) {
    let parent = dom.parent(elem).expect("elem has a parent");
    let idx = dom
        .index_in_parent(elem)
        .expect("elem is a child of its parent");
    let siblings = dom.children(parent);
    if let Some(&next) = siblings.get(idx + 1) {
        if matches!(dom.node(next).kind, NodeKind::Text(_)) {
            dom.node_mut(next).kind = NodeKind::Text(text.to_string());
            return;
        }
    }
    let t = dom.new_text(text);
    dom.insert_child(parent, idx + 1, t);
}

/// lxml's `elem.tail = None`: removes the text immediately following
/// `elem`, if any.
fn clear_tail(dom: &mut Dom, elem: NodeId) {
    let Some(parent) = dom.parent(elem) else {
        return;
    };
    let Some(idx) = dom.index_in_parent(elem) else {
        return;
    };
    let siblings = dom.children(parent);
    if let Some(&next) = siblings.get(idx + 1) {
        if matches!(dom.node(next).kind, NodeKind::Text(_)) {
            dom.detach(next);
        }
    }
}

/// Splits a colon-separated index entry's link `a` (currently holding
/// all of `parts`, joined) into one `<span>` per leading part plus a
/// final span wrapping `a` itself (now showing only the last part),
/// each indented `1.5em` deeper than the last. `ldict` records each
/// span's nesting depth for [`find_match`]/[`merge_blocks`] to compare
/// later. See the module docs for the real, reproduced bug in the
/// final `ldict` entry.
///
/// Port of `split_up_block`. Its `block`/`text` parameters are dropped
/// -- neither is referenced anywhere in the Python body.
fn split_up_block(dom: &mut Dom, a: NodeId, parts: &[String], ldict: &mut HashMap<NodeId, usize>) {
    let prefix_list = &parts[..parts.len() - 1];
    set_leading_text(dom, a, parts.last().expect("parts is non-empty"));
    let parent = dom.parent(a).expect("a has a parent");

    for (i, p) in prefix_list.iter().enumerate() {
        let m = 1.5 * i as f64;
        let span = dom.new_element("span");
        dom.node_mut(span).attrs.insert(
            "style".to_string(),
            format!("display:block; margin-left: {}em", format_g3(m)),
        );
        ldict.insert(span, i);
        dom.append_child(parent, span);
        set_leading_text(dom, span, p);
    }

    let last_i = prefix_list.len() - 1;
    let span = dom.new_element("span");
    dom.node_mut(span).attrs.insert(
        "style".to_string(),
        format!(
            "display:block; margin-left: {}em",
            format_g3((last_i as f64 + 1.0) * 1.5)
        ),
    );
    dom.append_child(parent, span);
    dom.append_child(span, a);
    // See the module docs: this reproduces a real upstream bug, not a
    // typo -- Python's shadowed `prefix` is a string here, and
    // `len(prefix)` is its character count, not `prefix_list.len()`.
    let shadowed_len = prefix_list
        .last()
        .expect("checked non-empty above")
        .chars()
        .count();
    ldict.insert(span, shadowed_len);
}

/// Whether `prev_block`'s child at `pind` is at a nesting depth that
/// has a *sibling* (later child of `prev_block`, one level deeper)
/// with the same text content as `nextent` -- i.e. whether `nextent`
/// already has a matching entry somewhere under `prev_block` that
/// [`merge_blocks`] should merge into, rather than appending a new
/// one. Scans forward only until a sibling at `pind`'s own depth (or
/// shallower) is found, since anything past that no longer nests
/// under `pind`.
///
/// Port of `find_match`. Python's `-1` "not found" sentinel becomes
/// `None` throughout (this crate's depths are `usize`, so `-1` isn't
/// representable, and every comparison Python does against it --
/// "missing implies smaller than any real depth" -- holds the same
/// way against `None` here).
fn find_match(
    dom: &Dom,
    prev_block: NodeId,
    pind: usize,
    nextent: NodeId,
    ldict: &HashMap<NodeId, usize>,
) -> Option<usize> {
    let prev_children = dom.children(prev_block);
    let cur_child = prev_children[pind];
    let curlevel = *ldict.get(&cur_child)?;

    for p in (pind + 1)..prev_children.len() {
        let child = prev_children[p];
        let Some(&trylev) = ldict.get(&child) else {
            return None;
        };
        if trylev <= curlevel {
            return None;
        }
        if trylev > curlevel + 1 {
            continue;
        }
        if dom.text_content(child) == dom.text_content(nextent) {
            return Some(p);
        }
    }
    None
}

/// Merges `nent`'s first link into `pent`: if `pent` already has a
/// link, appends `nent`'s as a `", "`-separated sibling after it;
/// otherwise `nent`'s link becomes `pent`'s sole content.
///
/// Port of `add_link`. Its `ldict` parameter is dropped -- unused in
/// the Python body.
fn add_link(dom: &mut Dom, pent: NodeId, nent: NodeId) {
    let Some(na) = dom.find_all_tag(nent, "a").into_iter().next() else {
        return;
    };
    let pa_list = dom.find_all_tag(pent, "a");
    if let Some(&pa) = pa_list.last() {
        set_tail(dom, pa, ", ");
        let parent = dom.parent(pa).expect("pa has a parent");
        let idx = dom
            .index_in_parent(pa)
            .expect("pa is a child of its parent");
        dom.insert_child(parent, idx + 1, na);
    } else {
        set_leading_text(dom, pent, "");
        dom.append_child(pent, na);
    }
}

/// The recursive merge step: walks `next_block`'s structure
/// (`next_path_len` deep) alongside `prev_block`'s, following matching
/// entries ([`find_match`]) as far down as they go, merging the link
/// once both sides bottom out ([`add_link`]), or -- once no further
/// match exists -- moving every remaining level of `next_block` over
/// to become new siblings under `prev_block` at that point and
/// discarding the now-empty (or partially-emptied) `next_block`.
///
/// The move loop relies on the same lxml behavior
/// [`crate::dom::Dom::insert_child`] replicates: inserting a node
/// elsewhere detaches it from wherever it was, so re-reading
/// `next_block`'s children at the same fixed index on every iteration
/// naturally walks its shrinking child list -- not an infinite loop,
/// even though `nind` itself is never incremented inside it.
///
/// Port of `merge_blocks`. Its `next_path` parameter (the full
/// `path_map[block]` list) is narrowed to just `next_path_len` -- only
/// its length is ever read in the Python body, never its contents.
fn merge_blocks(
    dom: &mut Dom,
    prev_block: NodeId,
    next_block: NodeId,
    pind: usize,
    nind: usize,
    next_path_len: usize,
    ldict: &mut HashMap<NodeId, usize>,
) {
    if next_path_len == nind + 1 {
        let pent = dom.children(prev_block)[pind];
        let nextent = dom.children(next_block)[nind];
        add_link(dom, pent, nextent);
        return;
    }

    let nind = nind + 1;
    let nextent = dom.children(next_block)[nind];
    if let Some(prevent) = find_match(dom, prev_block, pind, nextent, ldict) {
        merge_blocks(
            dom,
            prev_block,
            next_block,
            prevent,
            nind,
            next_path_len,
            ldict,
        );
        return;
    }

    let mut pind = pind;
    loop {
        let next_children = dom.children(next_block);
        let Some(&child) = next_children.get(nind) else {
            break;
        };
        pind += 1;
        dom.insert_child(prev_block, pind, child);
    }
    dom.detach(next_block);
}

/// Turns a reversed-document-order list of raw index-entry `blocks`
/// (each a `<p>`-like element whose sole descendant link's text --
/// possibly colon-separated into a path, e.g. `"Characters: Alice"` --
/// names the entry) into a properly nested index: colon-separated
/// entries are split into indented spans ([`split_up_block`]), each
/// `<br>`'s trailing text is cleared, and consecutive blocks sharing a
/// top-level path segment are merged into one via [`merge_blocks`].
///
/// Every block is assumed to contain at least one link (`<a>`); a
/// block with none is skipped rather than reproducing Python's
/// `IndexError` crash on `a[0]`, since there is nothing useful to
/// reproduce about it.
///
/// Port of `polish_index_markup`. Its `index` parameter is dropped --
/// unused in the Python body.
pub fn polish_index_markup(dom: &mut Dom, blocks: &[NodeId]) {
    let mut path_map: HashMap<NodeId, Vec<String>> = HashMap::new();
    let mut ldict: HashMap<NodeId, usize> = HashMap::new();

    for &block in blocks {
        let cls = dom
            .node(block)
            .attrs
            .get("class")
            .cloned()
            .unwrap_or_default();
        let new_cls = format!("{cls} index-entry");
        dom.node_mut(block)
            .attrs
            .insert("class".to_string(), new_cls.trim_start().to_string());

        let a = dom.find_all_tag(block, "a").into_iter().next();
        let text = a
            .map(|a| dom.text_content(a).trim().to_string())
            .unwrap_or_default();

        if text.contains(':') {
            let parts: Vec<String> = text
                .split(':')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect();
            let split = parts.len() > 1;
            path_map.insert(block, parts.clone());
            if split {
                if let Some(a) = a {
                    split_up_block(dom, a, &parts, &mut ldict);
                }
            }
        } else {
            path_map.insert(block, vec![text.clone()]);
            if let Some(a) = a {
                let parent = dom.parent(a).expect("a has a parent");
                let span = dom.new_element("span");
                dom.node_mut(span).attrs.insert(
                    "style".to_string(),
                    "display:block; margin-left: 0em".to_string(),
                );
                dom.append_child(parent, span);
                dom.append_child(span, a);
                ldict.insert(span, 0);
            }
        }

        for br in dom.find_all_tag(block, "br") {
            clear_tail(dom, br);
        }
    }

    let Some((&first, rest)) = blocks.split_first() else {
        return;
    };
    let mut prev_block = first;
    for &block in rest {
        let pp = &path_map[&prev_block];
        let pn = &path_map[&block];
        if pp[0] == pn[0] {
            let pn_len = pn.len();
            merge_blocks(dom, prev_block, block, 0, 0, pn_len, &mut ldict);
        } else {
            prev_block = block;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::Dom;

    fn new_a(dom: &mut Dom, parent: NodeId, href: &str, text: &str) -> NodeId {
        let a = dom.new_element("a");
        dom.node_mut(a)
            .attrs
            .insert("href".to_string(), href.to_string());
        let t = dom.new_text(text);
        dom.append_child(a, t);
        dom.append_child(parent, a);
        a
    }

    fn block_with_link(dom: &mut Dom, root: NodeId, text: &str, href: &str) -> NodeId {
        let block = dom.new_element("p");
        dom.append_child(root, block);
        new_a(dom, block, href, text);
        block
    }

    mod polish_index_markup_tests {
        use super::*;

        #[test]
        fn a_plain_entry_gets_wrapped_in_a_zero_indent_span() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let block = block_with_link(&mut dom, root, "Alice", "#idx1");

            polish_index_markup(&mut dom, &[block]);

            assert_eq!(
                dom.node(block).attrs.get("class").map(String::as_str),
                Some("index-entry")
            );
            let span = dom.find_all_tag(block, "span").into_iter().next().unwrap();
            assert!(dom
                .node(span)
                .attrs
                .get("style")
                .unwrap()
                .contains("margin-left: 0em"));
            assert_eq!(dom.find_all_tag(span, "a").len(), 1);
        }

        #[test]
        fn a_colon_separated_entry_is_split_into_indented_spans() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let block = block_with_link(&mut dom, root, "Characters: Alice", "#idx1");

            polish_index_markup(&mut dom, &[block]);

            let spans = dom.find_all_tag(block, "span");
            assert_eq!(
                spans.len(),
                2,
                "one prefix span plus the final wrapping span"
            );
            assert_eq!(dom.text_content(spans[0]).trim(), "Characters");
            let last_span_a = dom.find_all_tag(spans[1], "a");
            assert_eq!(last_span_a.len(), 1);
            assert_eq!(dom.text_content(last_span_a[0]), "Alice");
        }

        #[test]
        fn existing_class_is_preserved_alongside_index_entry() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let block = block_with_link(&mut dom, root, "Alice", "#idx1");
            dom.node_mut(block)
                .attrs
                .insert("class".to_string(), "foo".to_string());

            polish_index_markup(&mut dom, &[block]);

            assert_eq!(
                dom.node(block).attrs.get("class").map(String::as_str),
                Some("foo index-entry")
            );
        }

        #[test]
        fn a_br_s_trailing_text_is_cleared() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let block = block_with_link(&mut dom, root, "Alice", "#idx1");
            let br = dom.new_element("br");
            dom.append_child(block, br);
            let stray = dom.new_text("stray");
            dom.append_child(block, stray);

            polish_index_markup(&mut dom, &[block]);

            // The stray text right after `br` is gone; only the `<a>`'s
            // own text (moved into a wrapping span) remains.
            assert_eq!(dom.text_content(block).trim(), "Alice");
        }

        #[test]
        fn two_entries_sharing_a_top_level_segment_merge_into_one() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let b1 = block_with_link(&mut dom, root, "Characters: Alice", "#a1");
            let b2 = block_with_link(&mut dom, root, "Characters: Bob", "#a2");

            polish_index_markup(&mut dom, &[b1, b2]);

            // b2 was merged into b1 and detached from the tree.
            assert!(dom.parent(b2).is_none());
            let links = dom.find_all_tag(b1, "a");
            assert_eq!(links.len(), 2);
            // "Bob" before "Alice", not the reverse -- a direct
            // consequence of the reproduced `split_up_block` bug (see
            // the module docs): with a 10-character shared prefix
            // ("Characters"), `find_match` never finds b1's existing
            // "Alice" entry a shallow-enough match for b2's "Bob", so
            // `merge_blocks` falls through to its "insert the rest of
            // b2 into b1 at this position" branch, which inserts
            // *before* the entry already there.
            assert_eq!(dom.text_content(links[0]), "Bob");
            assert_eq!(dom.text_content(links[1]), "Alice");
        }

        #[test]
        fn two_entries_with_different_top_level_segments_stay_separate() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let b1 = block_with_link(&mut dom, root, "Characters: Alice", "#a1");
            let b2 = block_with_link(&mut dom, root, "Places: Wonderland", "#a2");

            polish_index_markup(&mut dom, &[b1, b2]);

            assert!(dom.parent(b1).is_some());
            assert!(dom.parent(b2).is_some());
        }

        #[test]
        fn a_third_level_shared_prefix_merges_the_deepest_matching_link() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let b1 = block_with_link(&mut dom, root, "A: B: One", "#a1");
            let b2 = block_with_link(&mut dom, root, "A: B: Two", "#a2");

            polish_index_markup(&mut dom, &[b1, b2]);

            assert!(dom.parent(b2).is_none());
            // The shared "A" (depth 0) and "B" (depth 1) prefixes
            // correctly match, recursing one level deep -- but at
            // that point the *same* reproduced `split_up_block` bug
            // (this time on the 1-character prefix "B", coincidentally
            // giving depth 1 instead of the intended 2) makes b1's own
            // final entry look like it's at the *same* depth as "B",
            // not one deeper, so the second-level match fails and
            // "Two" lands as a new sibling span next to "One" rather
            // than merging into it. Both links still end up under b1.
            let links = dom.find_all_tag(b1, "a");
            assert_eq!(links.len(), 2);
            assert_eq!(dom.text_content(links[0]), "Two");
            assert_eq!(dom.text_content(links[1]), "One");
        }
    }

    mod get_applicable_xe_fields_tests {
        use super::*;
        use roxmltree::Document;

        fn xe<'a, 'i>(
            text: &str,
            entry_type: Option<&str>,
            start_elem: Node<'a, 'i>,
        ) -> XeField<'a, 'i> {
            XeField {
                text: text.to_string(),
                entry_type: entry_type.map(str::to_string),
                page_number_text: None,
                anchor: "index-1".to_string(),
                start_elem,
            }
        }

        #[test]
        fn no_filters_returns_everything() {
            let doc = Document::parse("<w:document xmlns:w=\"x\"><w:p/></w:document>").unwrap();
            let start = doc.root_element();
            let index = IndexField::default();
            let fields = vec![xe("Alice", None, start), xe("Bob", None, start)];
            let ns = DocxNamespace::default();

            let result = get_applicable_xe_fields(&index, fields, &ns);
            assert_eq!(result.len(), 2);
        }

        #[test]
        fn entry_type_filters_to_matching_fields_only() {
            let doc = Document::parse("<w:document xmlns:w=\"x\"><w:p/></w:document>").unwrap();
            let start = doc.root_element();
            let index = IndexField {
                entry_type: Some("main".to_string()),
                ..Default::default()
            };
            let fields = vec![
                xe("Alice", Some("main"), start),
                xe("Bob", Some("sub"), start),
            ];
            let ns = DocxNamespace::default();

            let result = get_applicable_xe_fields(&index, fields, &ns);
            assert_eq!(result.len(), 1);
            assert_eq!(result[0].text, "Alice");
        }

        #[test]
        fn letter_range_filters_by_first_character() {
            let doc = Document::parse("<w:document xmlns:w=\"x\"><w:p/></w:document>").unwrap();
            let start = doc.root_element();
            let index = IndexField {
                letter_range: Some("A-M".to_string()),
                ..Default::default()
            };
            let fields = vec![xe("Alice", None, start), xe("Zeta", None, start)];
            let ns = DocxNamespace::default();

            let result = get_applicable_xe_fields(&index, fields, &ns);
            assert_eq!(result.len(), 1);
            assert_eq!(result[0].text, "Alice");
        }

        #[test]
        fn bookmark_restricts_to_entries_contained_in_the_named_bookmark() {
            let doc = Document::parse(
                r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
                     <w:bookmarkStart w:name="scope"><w:inner/></w:bookmarkStart>
                     <w:p/>
                   </w:document>"#,
            )
            .unwrap();
            let ns = DocxNamespace::default();
            let inner = ns
                .descendants(doc.root_element(), &["w:inner"])
                .into_iter()
                .next()
                .unwrap();
            let outer_p = ns
                .descendants(doc.root_element(), &["w:p"])
                .into_iter()
                .next()
                .unwrap();

            let index = IndexField {
                bookmark: Some("scope".to_string()),
                ..Default::default()
            };
            let fields = vec![xe("Inside", None, inner), xe("Outside", None, outer_p)];

            let result = get_applicable_xe_fields(&index, fields, &ns);
            assert_eq!(result.len(), 1);
            assert_eq!(result[0].text, "Inside");
        }
    }
}
