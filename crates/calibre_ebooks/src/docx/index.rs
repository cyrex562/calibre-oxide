//! Port of `old_src/src/calibre/ebooks/docx/index.py` (issue #293),
//! now fully ported: [`get_applicable_xe_fields`], [`process_index`]
//! (with its own private `make_block`/`add_xe` helpers), and the
//! recursive block-merge algorithm ([`split_up_block`], [`find_match`],
//! [`add_link`], [`merge_blocks`], [`polish_index_markup`]) that turns
//! generated index entries into a properly nested index.
//!
//! `make_block`/`add_xe`/`process_index` were previously thought to
//! need the same open architectural question as `fields.rs`'s
//! `parse_xe` (real `crate::xmltree` migration vs. a tracked
//! side-table) -- Python inserts synthetic `w:p`/`w:pPr`/`w:pStyle`/
//! `w:r`/`w:t`/`w:br` elements into the *source* tree so its own
//! already-running main body walk re-encounters and converts them
//! like any other paragraph. Tracing through exactly how those blocks
//! get consumed (`Fields.polish_markup`'s `object_map` lookup) found
//! that reframing wasn't actually necessary: since this port builds
//! HTML directly rather than round-tripping through a re-walked
//! source tree, [`process_index`] just builds the equivalent `<p>`/
//! `<a>` HTML straight into [`crate::dom::Dom`] itself -- no synthetic
//! source node, and no `crate::xmltree`, needed at all. `parse_xe`'s
//! own synthetic bookmark (`fields.rs`, issue #290) is expected to
//! reframe the same way -- see its module docs -- but remains
//! unported here; [`process_index`] accepts already-resolved
//! [`XeField`]s (with their `anchor` id already assigned) as an input,
//! not something it computes itself.
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

/// A new, empty `<p>` (with a `class` matching `style_class`, when
/// given) inserted into `parent` at `pos`.
///
/// Port of `make_block`. Python builds a `w:p > w:r > w:t` *source*
/// skeleton instead, so the rest of `Convert.__call__`'s already-running
/// main body walk would style-resolve and convert it exactly like any
/// other paragraph once it re-encounters the tree. This port has no
/// such walk to re-trigger -- [`process_index`] builds the equivalent
/// HTML directly, so there's no source node to synthesize here at
/// all; see the module docs.
fn make_block(dom: &mut Dom, style_class: Option<&str>, parent: NodeId, pos: usize) -> NodeId {
    let p = dom.new_element("p");
    if let Some(cls) = style_class {
        dom.node_mut(p)
            .attrs
            .insert("class".to_string(), cls.to_string());
    }
    dom.insert_child(parent, pos, p);
    p
}

/// Fills `p` (from [`make_block`]) with `xe`'s entry: an
/// `<a href="#{xe.anchor}">` holding `xe.text` (or a single space when
/// empty, matching Python's `xe.get('text') or ' '`), an optional
/// trailing `" [{page_number_text}]"` run, and a `<br>` -- exactly the
/// shape [`polish_index_markup`]/`split_up_block`'s own
/// `dom.find_all_tag(block, "a")` lookup expects (a link whose text is
/// the entry's own, possibly colon-separated, hierarchy).
///
/// Port of `add_xe`. `xe.anchor` is used as the link's `href` directly
/// (`#{anchor}`) rather than through Python's `hyperlink_fields`
/// indirection (a synthetic hyperlink-field entry resolved later by
/// generic field-hyperlink machinery): `xe.anchor` is a
/// programmatically-generated, already-unique, already-valid id (not
/// user input needing `generate_anchor`'s sanitization the way a real
/// bookmark name would), so nothing later needs to resolve it -- it's
/// simply where `parse_xe`'s own (not yet ported, issue #290) synthetic
/// anchor assignment is expected to stamp a matching `id`.
fn add_xe(dom: &mut Dom, p: NodeId, xe: &XeField) {
    let a = dom.new_element("a");
    dom.node_mut(a)
        .attrs
        .insert("href".to_string(), format!("#{}", xe.anchor));
    let text = if xe.text.is_empty() { " " } else { &xe.text };
    let t = dom.new_text(text);
    dom.append_child(a, t);
    dom.append_child(p, a);

    if let Some(pt) = xe.page_number_text.as_deref().filter(|pt| !pt.is_empty()) {
        let extra = dom.new_text(&format!(" [{pt}]"));
        dom.append_child(p, extra);
    }

    let br = dom.new_element("br");
    dom.append_child(p, br);
}

/// Replaces an `INDEX` field's own placeholder with generated index
/// entries: one `<p>` per [`XeField`] [`get_applicable_xe_fields`]
/// leaves applicable, sorted by text (or, when `index.heading` is
/// set, grouped under single-letter headings first), each holding an
/// `add_xe`-built link ready for [`polish_index_markup`] to later
/// merge into a nested index.
///
/// Every block is inserted into `parent` at `pos + i` (`i` in final
/// document order) -- simpler than, but observably identical to,
/// Python's own trick of inserting every block at the *same* fixed
/// `pos` while iterating `items` in reverse (which relies on each
/// insertion pushing the previous one down one slot).
///
/// Unlike Python, which discovers `parent`/`pos` (and `styles[0]`,
/// this function's `old_heading_style`) itself by walking the
/// `INDEX` field's own old Word-generated `w:p` content and removing
/// it from the *source* tree, this takes them as parameters --
/// locating (and, since there's no such content on the HTML side to
/// remove, simply not converting) the field's own placeholder is the
/// not-yet-ported `Fields` orchestrator's job (issue #290), not this
/// function's.
///
/// The letter-heading grouping (`index.heading.is_some()`) approximates
/// `partition_by_first_letter`'s real ICU-collation-based grouping (which
/// can group visually-distinct characters -- accented variants, digit
/// forms -- under one ordinal) as case-insensitive first-character
/// grouping over a case-insensitive sort -- the same disclosed
/// simplification `categories.rs`'s own `sort_key_for_name` already
/// makes elsewhere in this crate. One further quirk reproduced as-is,
/// not fixed: Python only substitutes a heading's own first character
/// with the real group letter when `heading_text` itself already
/// starts with `'a'`/`'A'` (`text.lower().startswith('a')`) -- a
/// heading template that doesn't start with that specific letter (an
/// unconventional `\h` switch argument) renders unchanged for every
/// group, not just the ones that happen to start differently.
///
/// Port of `process_index`.
pub fn process_index<'a, 'i>(
    dom: &mut Dom,
    parent: NodeId,
    pos: usize,
    index: &IndexField,
    xe_fields: Vec<XeField<'a, 'i>>,
    old_heading_style: Option<&str>,
    ns: &DocxNamespace,
) -> Vec<NodeId> {
    let applicable = get_applicable_xe_fields(index, xe_fields, ns);
    if applicable.is_empty() {
        return Vec::new();
    }

    enum Item<'x, 'a, 'i> {
        Heading(String),
        Entry(&'x XeField<'a, 'i>),
    }

    let mut sorted = applicable;
    sorted.sort_by(|a, b| a.text.to_uppercase().cmp(&b.text.to_uppercase()));

    let heading_style = match old_heading_style {
        Some(s) => s.to_string(),
        None => "IndexHeading".to_string(),
    };

    let items: Vec<Item> = if index.heading.is_some() {
        let mut items = Vec::new();
        let mut last_letter: Option<String> = None;
        for xe in &sorted {
            let letter = xe
                .text
                .chars()
                .next()
                .map(|c| c.to_uppercase().collect::<String>())
                .unwrap_or_else(|| " ".to_string());
            if last_letter.as_deref() != Some(letter.as_str()) {
                items.push(Item::Heading(letter.clone()));
                last_letter = Some(letter);
            }
            items.push(Item::Entry(xe));
        }
        items
    } else {
        sorted.iter().map(Item::Entry).collect()
    };

    let mut blocks = Vec::with_capacity(items.len());
    for (i, item) in items.into_iter().enumerate() {
        match item {
            Item::Heading(letter) => {
                let p = make_block(dom, Some(&heading_style), parent, pos + i);
                let heading_text = index.heading.as_deref().unwrap_or_default();
                let text = if heading_text.to_lowercase().starts_with('a') {
                    let rest: String = heading_text.chars().skip(1).collect();
                    format!("{letter}{rest}")
                } else {
                    heading_text.to_string()
                };
                let t = dom.new_text(&text);
                dom.append_child(p, t);
                blocks.push(p);
            }
            Item::Entry(xe) => {
                let p = make_block(dom, None, parent, pos + i);
                add_xe(dom, p, xe);
                blocks.push(p);
            }
        }
    }

    blocks
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

    mod process_index_tests {
        use super::*;
        use roxmltree::Document;

        fn xe<'a, 'i>(
            text: &str,
            page_number_text: Option<&str>,
            start_elem: Node<'a, 'i>,
            anchor: &str,
        ) -> XeField<'a, 'i> {
            XeField {
                text: text.to_string(),
                entry_type: None,
                page_number_text: page_number_text.map(str::to_string),
                anchor: anchor.to_string(),
                start_elem,
            }
        }

        #[test]
        fn no_applicable_fields_returns_nothing() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let index = IndexField::default();
            let ns = DocxNamespace::default();

            let blocks = process_index(&mut dom, root, 0, &index, Vec::new(), None, &ns);
            assert!(blocks.is_empty());
        }

        #[test]
        fn entries_sort_by_text_and_become_linked_paragraphs() {
            let doc = Document::parse("<w:document xmlns:w=\"x\"/>").unwrap();
            let start = doc.root_element();
            let mut dom = Dom::empty();
            let root = dom.root;
            let index = IndexField::default();
            let fields = vec![
                xe("Bob", None, start, "idx2"),
                xe("Alice", None, start, "idx1"),
            ];
            let ns = DocxNamespace::default();

            let blocks = process_index(&mut dom, root, 0, &index, fields, None, &ns);

            assert_eq!(blocks.len(), 2);
            assert_eq!(
                dom.children(root),
                blocks,
                "inserted in sorted document order"
            );
            let links: Vec<_> = blocks
                .iter()
                .map(|&b| dom.find_all_tag(b, "a").into_iter().next().unwrap())
                .collect();
            assert_eq!(dom.text_content(links[0]), "Alice");
            assert_eq!(
                dom.node(links[0]).attrs.get("href").map(String::as_str),
                Some("#idx1")
            );
            assert_eq!(dom.text_content(links[1]), "Bob");
        }

        #[test]
        fn an_entry_with_no_text_gets_a_single_space() {
            let doc = Document::parse("<w:document xmlns:w=\"x\"/>").unwrap();
            let start = doc.root_element();
            let mut dom = Dom::empty();
            let root = dom.root;
            let index = IndexField::default();
            let fields = vec![xe("", None, start, "idx1")];
            let ns = DocxNamespace::default();

            let blocks = process_index(&mut dom, root, 0, &index, fields, None, &ns);

            let link = dom.find_all_tag(blocks[0], "a").into_iter().next().unwrap();
            assert_eq!(dom.text_content(link), " ");
        }

        #[test]
        fn page_number_text_is_appended_in_brackets_after_the_link() {
            let doc = Document::parse("<w:document xmlns:w=\"x\"/>").unwrap();
            let start = doc.root_element();
            let mut dom = Dom::empty();
            let root = dom.root;
            let index = IndexField::default();
            let fields = vec![xe("Alice", Some("5"), start, "idx1")];
            let ns = DocxNamespace::default();

            let blocks = process_index(&mut dom, root, 0, &index, fields, None, &ns);

            // Checked node-by-node, not via `Dom::text_content` (its
            // real, tracked, unfixed multi-node-order bug -- #296 --
            // would reverse "Alice" and " [5]").
            let link = dom.find_all_tag(blocks[0], "a").into_iter().next().unwrap();
            assert_eq!(dom.text_content(link), "Alice");
            let children = dom.children(blocks[0]);
            assert_eq!(children[0], link);
            assert_eq!(dom.text_content(children[1]), " [5]");
            assert_eq!(dom.find_all_tag(blocks[0], "br").len(), 1);
        }

        #[test]
        fn heading_groups_entries_under_a_single_letter_heading() {
            let doc = Document::parse("<w:document xmlns:w=\"x\"/>").unwrap();
            let start = doc.root_element();
            let mut dom = Dom::empty();
            let root = dom.root;
            let index = IndexField {
                heading: Some("A".to_string()),
                ..Default::default()
            };
            let fields = vec![
                xe("Apple", None, start, "idx1"),
                xe("Banana", None, start, "idx2"),
                xe("Avocado", None, start, "idx3"),
            ];
            let ns = DocxNamespace::default();

            let blocks = process_index(&mut dom, root, 0, &index, fields, None, &ns);

            // Two headings ("A", "B") plus three entries.
            assert_eq!(blocks.len(), 5);
            assert_eq!(dom.text_content(blocks[0]), "A");
            assert!(dom.find_all_tag(blocks[0], "a").is_empty());
            assert_eq!(
                dom.node(blocks[0]).attrs.get("class").map(String::as_str),
                Some("IndexHeading")
            );
            assert_eq!(dom.text_content(blocks[1]), "Apple");
            assert_eq!(dom.text_content(blocks[2]), "Avocado");
            assert_eq!(dom.text_content(blocks[3]), "B");
            assert_eq!(dom.text_content(blocks[4]), "Banana");
        }

        #[test]
        fn old_heading_style_overrides_the_default_index_heading_class() {
            let doc = Document::parse("<w:document xmlns:w=\"x\"/>").unwrap();
            let start = doc.root_element();
            let mut dom = Dom::empty();
            let root = dom.root;
            let index = IndexField {
                heading: Some("A".to_string()),
                ..Default::default()
            };
            let fields = vec![xe("Apple", None, start, "idx1")];
            let ns = DocxNamespace::default();

            let blocks = process_index(&mut dom, root, 0, &index, fields, Some("MyHeading"), &ns);

            assert_eq!(
                dom.node(blocks[0]).attrs.get("class").map(String::as_str),
                Some("MyHeading")
            );
        }

        #[test]
        fn heading_text_starting_with_a_gets_its_first_char_replaced_per_group() {
            // Reproduced quirk: only fires because "A..." starts with
            // 'a' -- see the module docs on `process_index`.
            let doc = Document::parse("<w:document xmlns:w=\"x\"/>").unwrap();
            let start = doc.root_element();
            let mut dom = Dom::empty();
            let root = dom.root;
            let index = IndexField {
                heading: Some("Az".to_string()),
                ..Default::default()
            };
            let fields = vec![
                xe("Apple", None, start, "idx1"),
                xe("Banana", None, start, "idx2"),
            ];
            let ns = DocxNamespace::default();

            let blocks = process_index(&mut dom, root, 0, &index, fields, None, &ns);

            assert_eq!(
                dom.text_content(blocks[0]),
                "Az",
                "the 'A' got replaced with 'A'"
            );
            assert_eq!(
                dom.text_content(blocks[2]),
                "Bz",
                "the 'A' got replaced with 'B'"
            );
        }

        #[test]
        fn heading_text_not_starting_with_a_is_never_substituted() {
            let doc = Document::parse("<w:document xmlns:w=\"x\"/>").unwrap();
            let start = doc.root_element();
            let mut dom = Dom::empty();
            let root = dom.root;
            let index = IndexField {
                heading: Some("-".to_string()),
                ..Default::default()
            };
            let fields = vec![
                xe("Apple", None, start, "idx1"),
                xe("Banana", None, start, "idx2"),
            ];
            let ns = DocxNamespace::default();

            let blocks = process_index(&mut dom, root, 0, &index, fields, None, &ns);

            assert_eq!(dom.text_content(blocks[0]), "-");
            assert_eq!(dom.text_content(blocks[2]), "-");
        }

        #[test]
        fn blocks_are_inserted_at_the_given_position_not_only_appended() {
            let doc = Document::parse("<w:document xmlns:w=\"x\"/>").unwrap();
            let start = doc.root_element();
            let mut dom = Dom::empty();
            let root = dom.root;
            let before = dom.new_element("p");
            dom.append_child(root, before);
            let after = dom.new_element("p");
            dom.append_child(root, after);
            let index = IndexField::default();
            let fields = vec![xe("Alice", None, start, "idx1")];
            let ns = DocxNamespace::default();

            let blocks = process_index(&mut dom, root, 1, &index, fields, None, &ns);

            assert_eq!(dom.children(root), vec![before, blocks[0], after]);
        }
    }
}
