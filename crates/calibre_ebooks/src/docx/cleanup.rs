//! Port of `old_src/src/calibre/ebooks/docx/cleanup.py`, in full
//! (issue #291): every pass `cleanup_markup` runs over the converted
//! HTML tree, plus the `if detect_cover:` block at the very end
//! ([`detect_cover`]) -- kept as a separate function rather than
//! folded into [`cleanup_markup`], since it needs real filesystem I/O
//! (`dest_dir`) the rest of the file doesn't, and (like every other
//! `docx/to_html.rs`-adjacent piece ported so far) there is no real
//! orchestrator yet to call it at the right point in the pipeline.
//! `calibre.utils.imghdr.identify` -- the one piece of real work this
//! needed -- was already fully ported (`calibre_utils::imghdr`), so
//! this only had to port the detection logic *around* it.
//!
//! # Reused, not reinvented: `Dom::remove_promoting_children`
//!
//! Two of Python's trickiest-looking functions, [`lift`] and the
//! sole-span-unwrapping pass, turn out to be exactly
//! [`crate::dom::Dom::remove_promoting_children`] once translated out
//! of lxml's text/tail model. Python's `lift`:
//! ```python
//! def lift(span):
//!     parent = span.getparent()
//!     idx = parent.index(span)
//!     ...
//!     for child in reversed(span):
//!         parent.insert(idx, child)
//!     parent.remove(span)
//!     ... # span.text/span.tail redistribution
//! ```
//! is entirely about splicing `span`'s own content into its former
//! position and preserving whatever came right after it (`span.tail`)
//! -- exactly what `remove_promoting_children` already does, since
//! this crate's sibling-text-node model represents a leading text run,
//! each child's own trailing text, and the tail after the whole
//! element as one flat, already-correctly-ordered children list.
//! Nothing here needed to fragment that back into lxml's three-part
//! shape to reproduce the same observable result. (`docx/index.rs`'s
//! module docs make the same "sibling-text-node model eliminates
//! Python's `.text`/`.tail` bookkeeping" observation for `split_up_block`.)
//!
//! # A reproduced upstream bug
//!
//! The "merge consecutive spans with the same styling" pass never
//! flushes its final pending run:
//! ```python
//! current_run = []
//! for span in root.xpath('//span'):
//!     if not current_run:
//!         current_run.append(span)
//!     else:
//!         ...
//!         else:
//!             if len(current_run) > 1:
//!                 merge_run(current_run)
//!             current_run = [span]
//! # <- no trailing `if len(current_run) > 1: merge_run(current_run)` here
//! ```
//! A run only gets merged when a *non-mergeable* span ends it. If the
//! document's very last spans happen to be mergeable with each other,
//! that final run is silently never merged. Ported as-is.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::block_styles::Css;
use crate::dom::{Dom, NodeId, NodeKind};

pub const NBSP: &str = "\u{a0}";

const BLOCK_LIKE: &[&str] = &["p", "div", "h1", "h2", "h3", "h4", "h5", "h6"];

/// `id`'s `Element` children only, in document order -- lxml's
/// `len(elem)`/`elem[i]`/`for child in elem`, which (unlike this
/// crate's `Dom::children`) never count or yield text.
fn element_children(dom: &Dom, id: NodeId) -> Vec<NodeId> {
    dom.children(id)
        .into_iter()
        .filter(|&c| matches!(dom.node(c).kind, NodeKind::Element(_)))
        .collect()
}

/// The non-empty text immediately following `elem` among its siblings
/// -- lxml's `elem.tail` (truthy check already folded in: an empty or
/// absent tail both read as `None` here, matching `not elem.tail`).
fn tail_text(dom: &Dom, elem: NodeId) -> Option<String> {
    match dom.next_sibling(elem) {
        Some(next) => match &dom.node(next).kind {
            NodeKind::Text(s) if !s.is_empty() => Some(s.clone()),
            _ => None,
        },
        None => None,
    }
}

/// lxml's `elem.tail = text`.
fn set_tail(dom: &mut Dom, elem: NodeId, text: &str) {
    if let Some(next) = dom.next_sibling(elem) {
        if matches!(dom.node(next).kind, NodeKind::Text(_)) {
            dom.node_mut(next).kind = NodeKind::Text(text.to_string());
            return;
        }
    }
    let parent = dom.parent(elem).expect("elem has a parent");
    let idx = dom
        .index_in_parent(elem)
        .expect("elem is a child of its parent");
    let t = dom.new_text(text);
    dom.insert_child(parent, idx + 1, t);
}

/// lxml's `elem.text = text`.
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

/// Port of `append_text`: appends `text` to whatever's already at the
/// very end of `parent`'s content -- extending a trailing text node if
/// there is one, else adding a new one. This single rule covers both
/// of Python's cases (`parent.text` when `parent` has no element
/// children yet, `parent[-1].tail` when it does), since in this
/// crate's flat sibling model there is no structural difference
/// between the two -- both are simply "the last item in `parent`'s
/// children list, if it's text".
fn append_text(dom: &mut Dom, parent: NodeId, text: &str) {
    if text.is_empty() {
        return;
    }
    let children = dom.children(parent);
    if let Some(&last) = children.last() {
        if let NodeKind::Text(s) = &dom.node(last).kind {
            let mut new_s = s.clone();
            new_s.push_str(text);
            dom.node_mut(last).kind = NodeKind::Text(new_s);
            return;
        }
    }
    let t = dom.new_text(text);
    dom.append_child(parent, t);
}

/// Appends `node` to `parent`, merging it into `parent`'s current last
/// child if both are text (via [`append_text`]) rather than ever
/// leaving two adjacent text-node siblings -- purely cosmetic (this
/// crate's `Dom::serialize`/`Dom::text_content` don't care either way)
/// but keeps [`merge`]'s output structurally tidy.
fn append_node(dom: &mut Dom, parent: NodeId, node: NodeId) {
    if let NodeKind::Text(s) = dom.node(node).kind.clone() {
        append_text(dom, parent, &s);
        dom.detach(node);
    } else {
        dom.append_child(parent, node);
    }
}

/// Port of `mergeable`: whether `current` can be folded into
/// `previous` without changing anything observable -- no text between
/// them, identical `class`/`style`/`lang`/`dir`, no `id` on `current`,
/// and `current` really is `previous`'s immediate next sibling
/// *element* (nothing else, of any kind, in between).
fn mergeable(dom: &Dom, previous: NodeId, current: NodeId) -> bool {
    if tail_text(dom, previous).is_some() || tail_text(dom, current).is_some() {
        return false;
    }
    if dom.node(previous).attrs.get("class") != dom.node(current).attrs.get("class") {
        return false;
    }
    if dom
        .node(current)
        .attrs
        .get("id")
        .map(|s| !s.is_empty())
        .unwrap_or(false)
    {
        return false;
    }
    for attr in ["style", "lang", "dir"] {
        if dom.node(previous).attrs.get(attr) != dom.node(current).attrs.get(attr) {
            return false;
        }
    }
    dom.next_element_sibling(previous) == Some(current)
}

/// Port of `merge`: absorbs `span`'s entire content (and its own
/// trailing tail text) into `parent`, then removes `span`.
fn merge(dom: &mut Dom, parent: NodeId, span: NodeId) {
    for child in dom.children(span) {
        append_node(dom, parent, child);
    }
    if let Some(t) = tail_text(dom, span) {
        append_text(dom, parent, &t);
    }
    dom.detach(span);
}

/// Port of `merge_run`: folds every span after the first into it.
fn merge_run(dom: &mut Dom, run: &[NodeId]) {
    let parent = run[0];
    for &span in &run[1..] {
        merge(dom, parent, span);
    }
}

/// Port of `liftable`: a span's styling can move to its parent element
/// as-is if every one of its CSS properties is in a family
/// (`text-*`/`font-*`/`letter-*`/`color`/`background-*`) that doesn't
/// depend on being scoped to an inline span specifically.
fn liftable(css: &Css) -> bool {
    css.keys().all(|k| {
        let prefix = k.split('-').next().unwrap_or(k);
        matches!(prefix, "text" | "font" | "letter" | "color" | "background")
    })
}

/// Splits `data-docx-vert`-marked spans (superscript/subscript, set by
/// `convert_run`) into real `<sup>`/`<sub>` wrapping their whole
/// content. Port of `wrap_contents` + the `//span[@data-docx-vert]`
/// loop.
fn apply_vertical_align(dom: &mut Dom) {
    for span in dom.find_all_tag_global("span") {
        let Some(tag_name) = dom.node_mut(span).attrs.shift_remove("data-docx-vert") else {
            continue;
        };
        let wrapper = dom.new_element(&tag_name);
        for child in dom.children(span) {
            dom.append_child(wrapper, child);
        }
        dom.append_child(span, wrapper);
    }
}

/// Adds a thin space after a footnote-reference container's last
/// element if the *next* element sibling is itself another
/// note-reference container with content -- keeping consecutive
/// footnote markers from visually running together. Port of the
/// `data-noteref-container` loop.
fn separate_consecutive_noterefs(dom: &mut Dom, uuid: &str) {
    let elems: Vec<NodeId> = dom
        .preorder_elements(dom.root)
        .into_iter()
        .filter(|&n| {
            dom.node(n)
                .attrs
                .get("data-noteref-container")
                .map(String::as_str)
                == Some(uuid)
        })
        .collect();

    for elem in elems {
        dom.node_mut(elem)
            .attrs
            .shift_remove("data-noteref-container");
        let Some(parent) = dom.parent(elem) else {
            continue;
        };
        let siblings = element_children(dom, parent);
        let Some(idx) = siblings.iter().position(|&c| c == elem) else {
            continue;
        };
        if idx + 1 >= siblings.len() {
            continue;
        }
        let ns = siblings[idx + 1];
        if element_children(dom, ns).is_empty() {
            continue;
        }
        let ns_has_marker = dom
            .node(ns)
            .attrs
            .get("data-noteref-container")
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        if !ns_has_marker {
            continue;
        }
        let span_children = element_children(dom, elem);
        if let Some(&last) = span_children.last() {
            if tail_text(dom, last).is_none() {
                set_tail(dom, last, NBSP);
            }
        }
    }
}

/// Moves an `<hr>` that's the very last element inside its nearest
/// `p`/`h1`-`h6` block ancestor to become that block's own next
/// sibling, so it renders as a real horizontal rule between blocks
/// rather than stuck inside one. Port of the `//span/hr` loop.
fn move_trailing_hrs_out_of_paragraphs(dom: &mut Dom) {
    let hrs: Vec<NodeId> = dom
        .find_all_tag_global("hr")
        .into_iter()
        .filter(|&hr| dom.parent(hr).and_then(|p| dom.tag(p)) == Some("span"))
        .collect();

    for hr in hrs {
        let mut p = None;
        let mut cur = dom.parent(hr);
        while let Some(c) = cur {
            if BLOCK_LIKE.contains(&dom.tag(c).unwrap_or("")) {
                p = Some(c);
                break;
            }
            cur = dom.parent(c);
        }
        let Some(p) = p else { continue };

        let descendants: Vec<NodeId> = dom
            .preorder_elements(p)
            .into_iter()
            .filter(|&n| n != p)
            .collect();
        if descendants.last() != Some(&hr) {
            continue;
        }
        let Some(parent) = dom.parent(p) else {
            continue;
        };
        let idx = dom.index_in_parent(p).expect("p is a child of its parent");
        dom.insert_child(parent, idx + 1, hr);
        set_tail(dom, hr, "\n\t");
    }
}

/// Merges consecutive `<span>`s that share identical styling into one.
/// See the module docs for the real, reproduced bug in this pass
/// (its last pending run is never flushed).
fn merge_consecutive_spans(dom: &mut Dom) {
    let mut current_run: Vec<NodeId> = Vec::new();
    for span in dom.find_all_tag_global("span") {
        if current_run.is_empty() {
            current_run.push(span);
        } else {
            let last = *current_run.last().unwrap();
            if mergeable(dom, last, span) {
                current_run.push(span);
            } else {
                if current_run.len() > 1 {
                    merge_run(dom, &current_run);
                }
                current_run = vec![span];
            }
        }
    }
}

/// Normalizes `dir` on `<span>` children of block elements: an `rtl`
/// parent's non-`rtl` span children get an explicit `dir="ltr"`
/// (browsers don't otherwise inherit a *lack* of RTL correctly), and a
/// span whose `dir` now matches its parent's has the (now redundant)
/// attribute dropped.
fn normalize_span_dir(dom: &mut Dom) {
    let blocks: Vec<NodeId> = dom
        .preorder_elements(dom.root)
        .into_iter()
        .filter(|&n| BLOCK_LIKE.contains(&dom.tag(n).unwrap_or("")))
        .collect();

    for parent in blocks {
        let children = element_children(dom, parent);
        if children.is_empty() {
            continue;
        }
        let parent_dir = dom.node(parent).attrs.get("dir").cloned();
        let span_children: Vec<NodeId> = children
            .into_iter()
            .filter(|&c| dom.tag(c) == Some("span"))
            .collect();
        for child in span_children {
            let mut child_dir = dom.node(child).attrs.get("dir").cloned();
            if parent_dir.as_deref() == Some("rtl") && child_dir.as_deref() != Some("rtl") {
                child_dir = Some("ltr".to_string());
                dom.node_mut(child)
                    .attrs
                    .insert("dir".to_string(), "ltr".to_string());
            }
            if let Some(cd) = &child_dir {
                if !cd.is_empty() && Some(cd.as_str()) == parent_dir.as_deref() {
                    dom.node_mut(child).attrs.shift_remove("dir");
                }
            }
        }
    }
}

/// Unwraps a block element's sole `<span>` child directly into it,
/// when nothing would be lost by doing so (no id on the span, no other
/// text/content sharing the block, and the span's own styling is
/// `liftable`) -- absorbing the span's `class`/`lang`/`dir` onto the
/// block itself. Structurally this is exactly
/// `Dom::remove_promoting_children` (see the module docs); the
/// surrounding checks and attribute merging are what Python's version
/// spends most of its own length on.
fn unwrap_sole_span_children(dom: &mut Dom, class_map: &HashMap<String, Css>) {
    let candidates: Vec<NodeId> = dom
        .preorder_elements(dom.root)
        .into_iter()
        .filter(|&n| BLOCK_LIKE.contains(&dom.tag(n).unwrap_or("")))
        .filter(|&n| {
            element_children(dom, n)
                .iter()
                .filter(|&&c| dom.tag(c) == Some("span"))
                .count()
                == 1
        })
        .collect();

    for parent in candidates {
        let children = element_children(dom, parent);
        if children.len() != 1 {
            continue;
        }
        let span = children[0];

        let has_leading_text = matches!(dom.children(parent).first().map(|&c| dom.node(c).kind.clone()), Some(NodeKind::Text(s)) if !s.is_empty());
        if has_leading_text {
            continue;
        }
        if tail_text(dom, span).is_some() {
            continue;
        }
        if dom
            .node(span)
            .attrs
            .get("id")
            .map(|s| !s.is_empty())
            .unwrap_or(false)
        {
            continue;
        }

        let span_class = dom.node(span).attrs.get("class").cloned();
        let span_css = span_class
            .as_ref()
            .and_then(|c| class_map.get(c))
            .cloned()
            .unwrap_or_default();
        let span_dir = dom.node(span).attrs.get("dir").cloned();
        let parent_dir = dom.node(parent).attrs.get("dir").cloned();

        if !liftable(&span_css) {
            continue;
        }
        if let Some(sd) = &span_dir {
            if !sd.is_empty() && Some(sd.as_str()) != parent_dir.as_deref() {
                continue;
            }
        }

        if let Some(sc) = &span_class {
            let pclass = dom.node(parent).attrs.get("class").cloned();
            let new_class = match pclass {
                Some(p) if !p.is_empty() => format!("{p} {sc}"),
                _ => sc.clone(),
            };
            dom.node_mut(parent)
                .attrs
                .insert("class".to_string(), new_class);
        }
        let span_lang = dom.node(span).attrs.get("lang").cloned();

        dom.remove_promoting_children(span);

        if let Some(lang) = span_lang.filter(|s| !s.is_empty()) {
            dom.node_mut(parent).attrs.insert("lang".to_string(), lang);
        }
        if let Some(dir) = span_dir.filter(|s| !s.is_empty()) {
            dom.node_mut(parent).attrs.insert("dir".to_string(), dir);
        }
    }
}

/// Retags a span whose *only* styling is `font-style: italic` or
/// `font-weight: bold` into a real `<i>`/`<b>`, dropping its now-empty
/// `class`.
fn simplify_bold_italic_spans(dom: &mut Dom, class_map: &HashMap<String, Css>) {
    for span in dom.find_all_tag_global("span") {
        let (has_class, has_style, span_class) = {
            let attrs = &dom.node(span).attrs;
            (
                attrs.contains_key("class"),
                attrs.contains_key("style"),
                attrs.get("class").cloned(),
            )
        };
        if !has_class || has_style {
            continue;
        }
        let css = span_class
            .as_ref()
            .and_then(|c| class_map.get(c))
            .cloned()
            .unwrap_or_default();
        if css.len() != 1 {
            continue;
        }
        if css.get("font-style").map(String::as_str) == Some("italic") {
            dom.set_tag(span, "i");
            dom.node_mut(span).attrs.shift_remove("class");
        } else if css.get("font-weight").map(String::as_str) == Some("bold") {
            dom.set_tag(span, "b");
            dom.node_mut(span).attrs.shift_remove("class");
        }
    }
}

/// Port of `lift`: replaces `span` with its own content in place. See
/// the module docs for why this is exactly `remove_promoting_children`.
fn lift(dom: &mut Dom, span: NodeId) {
    dom.remove_promoting_children(span);
}

/// Removes every `<span>` with no `class`/`id`/`style`/`lang`/`dir` --
/// pure noise once nothing above needed it as a styling hook.
fn remove_unstyled_spans(dom: &mut Dom) {
    for span in dom.find_all_tag_global("span") {
        let has_any = {
            let attrs = &dom.node(span).attrs;
            ["class", "id", "style", "lang", "dir"]
                .iter()
                .any(|a| attrs.contains_key(*a))
        };
        if !has_any {
            lift(dom, span);
        }
    }
}

/// Converts `<p><br style="page-break-after:always"></p>` (Word's own
/// page-break idiom) into a `page-break-after:always` style on the
/// `<p>` itself, which every reader/renderer already knows how to
/// honor without depending on an otherwise-invisible `<br>`.
fn convert_page_break_paragraphs(dom: &mut Dom) {
    let ps: Vec<NodeId> = dom
        .find_all_tag_global("p")
        .into_iter()
        .filter(|&p| {
            element_children(dom, p).iter().any(|&c| {
                dom.tag(c) == Some("br")
                    && dom.node(c).attrs.get("style").map(String::as_str)
                        == Some("page-break-after:always")
            })
        })
        .collect();

    for p in ps {
        let elems = element_children(dom, p);
        if elems.len() != 1 {
            continue;
        }
        let br = elems[0];
        let tail_ok = tail_text(dom, br)
            .map(|t| t.trim().is_empty())
            .unwrap_or(true);
        if !tail_ok {
            continue;
        }

        // `p.remove(br)` in lxml drops br's own tail along with it.
        if let Some(t) = dom.next_sibling(br) {
            if matches!(dom.node(t).kind, NodeKind::Text(_)) {
                dom.detach(t);
            }
        }
        dom.detach(br);

        let mut style = dom.node(p).attrs.get("style").cloned().unwrap_or_default();
        if !style.is_empty() {
            style.push_str("; ");
        }
        style.push_str("page-break-after:always");
        dom.node_mut(p).attrs.insert("style".to_string(), style);

        let has_leading_text = matches!(dom.children(p).first().map(|&c| dom.node(c).kind.clone()), Some(NodeKind::Text(s)) if !s.is_empty());
        if !has_leading_text {
            set_leading_text(dom, p, NBSP);
        }
    }
}

/// The markup-cleanup passes `Convert.__call__` runs over the finished
/// HTML tree, in order -- everything in Python's `cleanup_markup`
/// except the `if detect_cover:` block ([`detect_cover`], see the
/// module docs). `class_map` is `styles.class_map()`; `uuid` is the
/// same per-document id `convert_p`/`convert_footnotes` use to mark
/// footnote-reference containers.
///
/// Port of `cleanup_markup`.
pub fn cleanup_markup(dom: &mut Dom, class_map: &HashMap<String, Css>, uuid: &str) {
    apply_vertical_align(dom);
    separate_consecutive_noterefs(dom, uuid);
    move_trailing_hrs_out_of_paragraphs(dom);
    merge_consecutive_spans(dom);
    normalize_span_dir(dom);
    unwrap_sole_span_children(dom, class_map);
    simplify_bold_italic_spans(dom, class_map);
    remove_unstyled_spans(dom);
    convert_page_break_paragraphs(dom);
}

/// How many elements before `tag`, in document-order traversal of the
/// document's first `<body>`, up to `limit` (returned once that many
/// have been counted without finding `tag`). `limit` is also the
/// fallback when there's no `<body>` at all.
///
/// Python falls off the end of its loop with an implicit `None` if
/// `tag` isn't actually a descendant of `<body>` -- a real crash risk
/// in its one caller, `before_count(...) < 5`, comparing `None` against
/// an `int`. This returns `limit` instead, which fails that same `< 5`
/// check the same way the crash would have prevented a cover from ever
/// being "detected" -- there's nothing useful to reproduce about the
/// crash itself.
///
/// Port of `before_count`.
fn before_count(dom: &Dom, tag: NodeId, limit: i64) -> i64 {
    let Some(body) = dom.find_first_tag_global("body") else {
        return limit;
    };
    let mut ans: i64 = 0;
    for elem in dom
        .preorder_elements(body)
        .into_iter()
        .filter(|&n| n != body)
    {
        if elem == tag {
            return ans;
        }
        ans += 1;
        if ans > limit {
            return limit;
        }
    }
    limit
}

/// Checks whether the document's first `<img>` (if it appears early
/// enough -- within the first 5 of the first 10 elements in `<body>`)
/// looks like a cover: its file exists under `dest_dir`, and its
/// dimensions (read via [`calibre_utils::imghdr::identify`]) give a
/// roughly-portrait aspect ratio (0.8-1.8) at a reasonable size (at
/// least ~400x400). If so, removes that `<img>` from the tree (a
/// detected cover doesn't also appear inline in the text) and returns
/// its path.
///
/// An unreadable or dimension-less image is treated as definitely not
/// a cover (`height as f64 / width as f64` naturally becomes `NaN` or
/// `inf` when a dimension is `0` -- both fail every range check below
/// -- matching Python's `except ZeroDivisionError: is_cover = False`
/// without needing a separate check).
///
/// Port of `cleanup_markup`'s `if detect_cover:` block.
pub fn detect_cover(dom: &mut Dom, dest_dir: &Path) -> Option<PathBuf> {
    let img = dom
        .find_all_tag_global("img")
        .into_iter()
        .find(|&i| dom.node(i).attrs.contains_key("src"))?;
    let src = dom.node(img).attrs.get("src")?.clone();
    let path = dest_dir.join(&src);
    if !path.exists() {
        return None;
    }
    if before_count(dom, img, 10) >= 5 {
        return None;
    }

    let (width, height) = match std::fs::read(&path) {
        Ok(data) => {
            let (_fmt, w, h) = calibre_utils::imghdr::identify(&data);
            (w, h)
        }
        Err(_) => (0, 0),
    };
    let ratio = height as f64 / width as f64;
    let is_cover = (0.8..=1.8).contains(&ratio) && (height * width) >= 160_000;
    if !is_cover {
        return None;
    }

    dom.detach(img);
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_span(dom: &mut Dom, parent: NodeId) -> NodeId {
        let span = dom.new_element("span");
        dom.append_child(parent, span);
        span
    }

    fn add_text_child(dom: &mut Dom, parent: NodeId, text: &str) {
        let t = dom.new_text(text);
        dom.append_child(parent, t);
    }

    mod apply_vertical_align_tests {
        use super::*;

        #[test]
        fn a_marked_span_gets_its_content_wrapped_in_the_named_tag() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let span = new_span(&mut dom, root);
            dom.node_mut(span)
                .attrs
                .insert("data-docx-vert".to_string(), "sup".to_string());
            add_text_child(&mut dom, span, "2");

            apply_vertical_align(&mut dom);

            assert!(!dom.node(span).attrs.contains_key("data-docx-vert"));
            let wrapper = dom.children(span)[0];
            assert_eq!(dom.tag(wrapper), Some("sup"));
            assert_eq!(dom.text_content(span), "2");
        }
    }

    mod merge_consecutive_spans_tests {
        use super::*;

        fn styled_span(dom: &mut Dom, parent: NodeId, class: &str, text: &str) -> NodeId {
            let span = new_span(dom, parent);
            dom.node_mut(span)
                .attrs
                .insert("class".to_string(), class.to_string());
            add_text_child(dom, span, text);
            span
        }

        #[test]
        fn two_identically_styled_adjacent_spans_merge() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let p = dom.new_element("p");
            dom.append_child(root, p);
            let s1 = styled_span(&mut dom, p, "c1", "a");
            styled_span(&mut dom, p, "c1", "b");
            // A trailing non-mergeable span forces the pending run to flush
            // (the trailing-run bug means the run above wouldn't otherwise
            // get merged at all -- see the module docs).
            styled_span(&mut dom, p, "c2", "z");

            merge_consecutive_spans(&mut dom);

            assert_eq!(
                dom.children(p).len(),
                2,
                "s1+s2 merged into one span, plus the trailing one"
            );
            assert_eq!(dom.text_content(s1), "ab");
        }

        #[test]
        fn differing_class_prevents_merging() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let p = dom.new_element("p");
            dom.append_child(root, p);
            styled_span(&mut dom, p, "c1", "a");
            styled_span(&mut dom, p, "c2", "b");
            styled_span(&mut dom, p, "c3", "z");

            merge_consecutive_spans(&mut dom);

            assert_eq!(dom.children(p).len(), 3);
        }

        #[test]
        fn a_final_mergeable_run_is_never_flushed() {
            // Reproduces the real upstream bug documented in the module
            // docs: nothing forces the LAST pending run to merge.
            let mut dom = Dom::empty();
            let root = dom.root;
            let p = dom.new_element("p");
            dom.append_child(root, p);
            styled_span(&mut dom, p, "c1", "a");
            styled_span(&mut dom, p, "c1", "b");

            merge_consecutive_spans(&mut dom);

            assert_eq!(
                dom.children(p).len(),
                2,
                "the trailing run was left unmerged"
            );
        }

        #[test]
        fn an_id_on_the_later_span_prevents_merging() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let p = dom.new_element("p");
            dom.append_child(root, p);
            styled_span(&mut dom, p, "c1", "a");
            let s2 = styled_span(&mut dom, p, "c1", "b");
            dom.node_mut(s2)
                .attrs
                .insert("id".to_string(), "anchor".to_string());
            styled_span(&mut dom, p, "c2", "z");

            merge_consecutive_spans(&mut dom);

            assert_eq!(dom.children(p).len(), 3, "s2's id blocks merging with s1");
        }
    }

    mod remove_unstyled_spans_tests {
        use super::*;

        #[test]
        fn a_bare_span_is_lifted_away() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let p = dom.new_element("p");
            dom.append_child(root, p);
            let span = new_span(&mut dom, p);
            add_text_child(&mut dom, span, "hello");

            remove_unstyled_spans(&mut dom);

            assert!(dom.parent(span).is_none());
            assert_eq!(dom.text_content(p), "hello");
        }

        #[test]
        fn a_styled_span_survives() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let p = dom.new_element("p");
            dom.append_child(root, p);
            let span = new_span(&mut dom, p);
            dom.node_mut(span)
                .attrs
                .insert("style".to_string(), "color:red".to_string());
            add_text_child(&mut dom, span, "hello");

            remove_unstyled_spans(&mut dom);

            assert!(dom.parent(span).is_some());
        }
    }

    mod simplify_bold_italic_spans_tests {
        use super::*;

        fn class_map_with(name: &str, css: &[(&str, &str)]) -> HashMap<String, Css> {
            let mut m = Css::new();
            for &(k, v) in css {
                m.insert(k.to_string(), v.to_string());
            }
            HashMap::from([(name.to_string(), m)])
        }

        #[test]
        fn an_italic_only_span_becomes_i() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let span = new_span(&mut dom, root);
            dom.node_mut(span)
                .attrs
                .insert("class".to_string(), "s1".to_string());
            let class_map = class_map_with("s1", &[("font-style", "italic")]);

            simplify_bold_italic_spans(&mut dom, &class_map);

            assert_eq!(dom.tag(span), Some("i"));
            assert!(!dom.node(span).attrs.contains_key("class"));
        }

        #[test]
        fn a_bold_only_span_becomes_b() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let span = new_span(&mut dom, root);
            dom.node_mut(span)
                .attrs
                .insert("class".to_string(), "s1".to_string());
            let class_map = class_map_with("s1", &[("font-weight", "bold")]);

            simplify_bold_italic_spans(&mut dom, &class_map);

            assert_eq!(dom.tag(span), Some("b"));
        }

        #[test]
        fn a_span_with_more_than_one_property_is_left_alone() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let span = new_span(&mut dom, root);
            dom.node_mut(span)
                .attrs
                .insert("class".to_string(), "s1".to_string());
            let class_map = class_map_with("s1", &[("font-weight", "bold"), ("color", "red")]);

            simplify_bold_italic_spans(&mut dom, &class_map);

            assert_eq!(dom.tag(span), Some("span"));
        }

        #[test]
        fn a_span_with_an_explicit_style_attr_is_skipped() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let span = new_span(&mut dom, root);
            dom.node_mut(span)
                .attrs
                .insert("class".to_string(), "s1".to_string());
            dom.node_mut(span)
                .attrs
                .insert("style".to_string(), "color:red".to_string());
            let class_map = class_map_with("s1", &[("font-style", "italic")]);

            simplify_bold_italic_spans(&mut dom, &class_map);

            assert_eq!(dom.tag(span), Some("span"));
        }
    }

    mod unwrap_sole_span_children_tests {
        use super::*;

        fn class_map_with(name: &str, css: &[(&str, &str)]) -> HashMap<String, Css> {
            let mut m = Css::new();
            for &(k, v) in css {
                m.insert(k.to_string(), v.to_string());
            }
            HashMap::from([(name.to_string(), m)])
        }

        #[test]
        fn a_liftable_sole_span_is_absorbed_by_its_parent() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let p = dom.new_element("p");
            dom.append_child(root, p);
            let span = new_span(&mut dom, p);
            dom.node_mut(span)
                .attrs
                .insert("class".to_string(), "c1".to_string());
            dom.node_mut(span)
                .attrs
                .insert("lang".to_string(), "en".to_string());
            add_text_child(&mut dom, span, "hello");
            let class_map = class_map_with("c1", &[("color", "red")]);

            unwrap_sole_span_children(&mut dom, &class_map);

            assert_eq!(
                dom.children(p).len(),
                1,
                "span replaced by its own text content"
            );
            assert_eq!(dom.text_content(p), "hello");
            assert_eq!(
                dom.node(p).attrs.get("class").map(String::as_str),
                Some("c1")
            );
            assert_eq!(
                dom.node(p).attrs.get("lang").map(String::as_str),
                Some("en")
            );
        }

        #[test]
        fn a_non_liftable_span_stays_wrapped() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let p = dom.new_element("p");
            dom.append_child(root, p);
            let span = new_span(&mut dom, p);
            dom.node_mut(span)
                .attrs
                .insert("class".to_string(), "c1".to_string());
            add_text_child(&mut dom, span, "hello");
            // `display` isn't in the liftable prefix set.
            let class_map = class_map_with("c1", &[("display", "inline-block")]);

            unwrap_sole_span_children(&mut dom, &class_map);

            assert_eq!(dom.children(p).len(), 1);
            assert_eq!(dom.tag(dom.children(p)[0]), Some("span"));
        }

        #[test]
        fn a_span_with_an_id_stays_wrapped() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let p = dom.new_element("p");
            dom.append_child(root, p);
            let span = new_span(&mut dom, p);
            dom.node_mut(span)
                .attrs
                .insert("id".to_string(), "anchor".to_string());
            add_text_child(&mut dom, span, "hello");

            unwrap_sole_span_children(&mut dom, &HashMap::new());

            assert_eq!(dom.tag(dom.children(p)[0]), Some("span"));
        }
    }

    mod convert_page_break_paragraphs_tests {
        use super::*;

        #[test]
        fn a_sole_page_break_br_becomes_a_style_on_the_paragraph() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let p = dom.new_element("p");
            dom.append_child(root, p);
            let br = dom.new_element("br");
            dom.node_mut(br)
                .attrs
                .insert("style".to_string(), "page-break-after:always".to_string());
            dom.append_child(p, br);

            convert_page_break_paragraphs(&mut dom);

            assert!(dom.children(p).iter().all(|&c| dom.tag(c) != Some("br")));
            assert_eq!(
                dom.node(p).attrs.get("style").map(String::as_str),
                Some("page-break-after:always")
            );
            assert_eq!(dom.text_content(p), NBSP);
        }

        #[test]
        fn an_existing_style_is_preserved_alongside_the_page_break() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let p = dom.new_element("p");
            dom.node_mut(p)
                .attrs
                .insert("style".to_string(), "color:red".to_string());
            dom.append_child(root, p);
            let br = dom.new_element("br");
            dom.node_mut(br)
                .attrs
                .insert("style".to_string(), "page-break-after:always".to_string());
            dom.append_child(p, br);

            convert_page_break_paragraphs(&mut dom);

            assert_eq!(
                dom.node(p).attrs.get("style").map(String::as_str),
                Some("color:red; page-break-after:always")
            );
        }

        #[test]
        fn existing_leading_text_survives_the_conversion_unchanged() {
            // Python's `len(p) == 1` check counts *element* children
            // only (lxml's own `len()` semantics) -- `p.text` isn't
            // part of that count, so a paragraph with leading text
            // AND a sole page-break `<br>` still converts, keeping
            // its text rather than replacing it with NBSP.
            let mut dom = Dom::empty();
            let root = dom.root;
            let p = dom.new_element("p");
            dom.append_child(root, p);
            add_text_child(&mut dom, p, "text");
            let br = dom.new_element("br");
            dom.node_mut(br)
                .attrs
                .insert("style".to_string(), "page-break-after:always".to_string());
            dom.append_child(p, br);

            convert_page_break_paragraphs(&mut dom);

            assert!(dom.children(p).iter().all(|&c| dom.tag(c) != Some("br")));
            assert_eq!(dom.text_content(p), "text");
            assert_eq!(
                dom.node(p).attrs.get("style").map(String::as_str),
                Some("page-break-after:always")
            );
        }

        #[test]
        fn a_second_element_sibling_of_the_br_blocks_conversion() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let p = dom.new_element("p");
            dom.append_child(root, p);
            let br = dom.new_element("br");
            dom.node_mut(br)
                .attrs
                .insert("style".to_string(), "page-break-after:always".to_string());
            dom.append_child(p, br);
            new_span(&mut dom, p);

            convert_page_break_paragraphs(&mut dom);

            assert!(dom.children(p).iter().any(|&c| dom.tag(c) == Some("br")));
        }
    }

    mod normalize_span_dir_tests {
        use super::*;

        #[test]
        fn an_ltr_span_under_an_rtl_parent_gets_an_explicit_dir() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let p = dom.new_element("p");
            dom.node_mut(p)
                .attrs
                .insert("dir".to_string(), "rtl".to_string());
            dom.append_child(root, p);
            let span = new_span(&mut dom, p);

            normalize_span_dir(&mut dom);

            assert_eq!(
                dom.node(span).attrs.get("dir").map(String::as_str),
                Some("ltr")
            );
        }

        #[test]
        fn a_span_matching_its_parents_dir_has_it_removed() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let p = dom.new_element("p");
            dom.node_mut(p)
                .attrs
                .insert("dir".to_string(), "ltr".to_string());
            dom.append_child(root, p);
            let span = new_span(&mut dom, p);
            dom.node_mut(span)
                .attrs
                .insert("dir".to_string(), "ltr".to_string());

            normalize_span_dir(&mut dom);

            assert!(!dom.node(span).attrs.contains_key("dir"));
        }
    }

    mod move_trailing_hrs_out_of_paragraphs_tests {
        use super::*;

        #[test]
        fn a_trailing_hr_moves_to_be_the_paragraphs_own_next_sibling() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let p = dom.new_element("p");
            dom.append_child(root, p);
            let span = new_span(&mut dom, p);
            let hr = dom.new_element("hr");
            dom.append_child(span, hr);

            move_trailing_hrs_out_of_paragraphs(&mut dom);

            assert_eq!(dom.parent(hr), Some(root));
            let root_children = dom.children(root);
            assert_eq!(
                &root_children[..2],
                &[p, hr],
                "hr becomes p's immediate next sibling"
            );
            assert_eq!(tail_text(&dom, hr).as_deref(), Some("\n\t"));
        }

        #[test]
        fn a_non_trailing_hr_stays_put() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let p = dom.new_element("p");
            dom.append_child(root, p);
            let span = new_span(&mut dom, p);
            let hr = dom.new_element("hr");
            dom.append_child(span, hr);
            new_span(&mut dom, p); // a sibling *after* span, so hr isn't p's last descendant

            move_trailing_hrs_out_of_paragraphs(&mut dom);

            assert_eq!(dom.parent(hr), Some(span));
        }
    }

    mod separate_consecutive_noterefs_tests {
        use super::*;

        #[test]
        fn two_adjacent_noteref_containers_get_a_separating_nbsp() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let a1 = dom.new_element("a");
            dom.node_mut(a1)
                .attrs
                .insert("data-noteref-container".to_string(), "u".to_string());
            dom.append_child(root, a1);
            let inner1 = dom.new_element("span");
            dom.append_child(a1, inner1);

            let a2 = dom.new_element("a");
            dom.node_mut(a2)
                .attrs
                .insert("data-noteref-container".to_string(), "u".to_string());
            dom.append_child(root, a2);
            let inner2 = dom.new_element("span");
            dom.append_child(a2, inner2);

            separate_consecutive_noterefs(&mut dom, "u");

            assert!(!dom.node(a1).attrs.contains_key("data-noteref-container"));
            assert_eq!(tail_text(&dom, inner1).as_deref(), Some(NBSP));
        }

        #[test]
        fn a_lone_noteref_container_gets_no_separator() {
            let mut dom = Dom::empty();
            let root = dom.root;
            let a1 = dom.new_element("a");
            dom.node_mut(a1)
                .attrs
                .insert("data-noteref-container".to_string(), "u".to_string());
            dom.append_child(root, a1);
            let inner1 = dom.new_element("span");
            dom.append_child(a1, inner1);

            separate_consecutive_noterefs(&mut dom, "u");

            assert!(tail_text(&dom, inner1).is_none());
        }
    }

    mod detect_cover_tests {
        use super::*;

        /// A minimal well-formed PNG with the given pixel dimensions,
        /// matching `calibre_utils::imghdr`'s own test fixture layout.
        fn png_bytes(width: u32, height: u32) -> Vec<u8> {
            let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
            data.extend_from_slice(&[0, 0, 0, 13]);
            data.extend_from_slice(b"IHDR");
            data.extend_from_slice(&width.to_be_bytes());
            data.extend_from_slice(&height.to_be_bytes());
            data
        }

        fn img(dom: &mut Dom, parent: NodeId, src: &str) -> NodeId {
            let img = dom.new_element("img");
            dom.node_mut(img)
                .attrs
                .insert("src".to_string(), src.to_string());
            dom.append_child(parent, img);
            img
        }

        #[test]
        fn a_portrait_image_early_in_the_body_is_detected_as_the_cover() {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("cover.png"), png_bytes(600, 800)).unwrap();

            let mut dom = Dom::empty();
            let body = dom.new_element("body");
            dom.append_child(dom.root, body);
            let the_img = img(&mut dom, body, "cover.png");

            let result = detect_cover(&mut dom, dir.path());

            assert_eq!(result, Some(dir.path().join("cover.png")));
            assert!(
                dom.parent(the_img).is_none(),
                "the cover img is removed from the tree"
            );
        }

        #[test]
        fn a_missing_file_is_not_a_cover() {
            let dir = tempfile::tempdir().unwrap();
            let mut dom = Dom::empty();
            let body = dom.new_element("body");
            dom.append_child(dom.root, body);
            img(&mut dom, body, "does-not-exist.png");

            assert_eq!(detect_cover(&mut dom, dir.path()), None);
        }

        #[test]
        fn a_small_image_is_not_a_cover() {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("tiny.png"), png_bytes(50, 60)).unwrap();

            let mut dom = Dom::empty();
            let body = dom.new_element("body");
            dom.append_child(dom.root, body);
            img(&mut dom, body, "tiny.png");

            assert_eq!(detect_cover(&mut dom, dir.path()), None);
        }

        #[test]
        fn a_wide_image_is_not_a_cover() {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("wide.png"), png_bytes(1000, 400)).unwrap();

            let mut dom = Dom::empty();
            let body = dom.new_element("body");
            dom.append_child(dom.root, body);
            img(&mut dom, body, "wide.png");

            assert_eq!(detect_cover(&mut dom, dir.path()), None);
        }

        #[test]
        fn an_image_buried_deep_in_the_body_is_not_a_cover() {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("cover.png"), png_bytes(600, 800)).unwrap();

            let mut dom = Dom::empty();
            let body = dom.new_element("body");
            dom.append_child(dom.root, body);
            for _ in 0..10 {
                new_span(&mut dom, body);
            }
            img(&mut dom, body, "cover.png");

            assert_eq!(detect_cover(&mut dom, dir.path()), None);
        }

        #[test]
        fn no_img_at_all_is_not_a_cover() {
            let dir = tempfile::tempdir().unwrap();
            let mut dom = Dom::empty();
            assert_eq!(detect_cover(&mut dom, dir.path()), None);
        }
    }

    mod before_count_tests {
        use super::*;

        #[test]
        fn no_body_returns_the_limit() {
            let dom = Dom::empty();
            assert_eq!(before_count(&dom, dom.root, 10), 10);
        }

        #[test]
        fn counts_elements_before_the_target() {
            let mut dom = Dom::empty();
            let body = dom.new_element("body");
            dom.append_child(dom.root, body);
            new_span(&mut dom, body);
            new_span(&mut dom, body);
            let target = new_span(&mut dom, body);

            assert_eq!(before_count(&dom, target, 10), 2);
        }

        #[test]
        fn a_target_past_the_limit_returns_the_limit() {
            let mut dom = Dom::empty();
            let body = dom.new_element("body");
            dom.append_child(dom.root, body);
            for _ in 0..15 {
                new_span(&mut dom, body);
            }
            let target = new_span(&mut dom, body);

            assert_eq!(before_count(&dom, target, 10), 10);
        }
    }
}
