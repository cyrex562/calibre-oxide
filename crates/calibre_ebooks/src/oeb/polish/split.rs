//! Port of `old_src/src/calibre/ebooks/oeb/polish/split.py`.
//!
//! Per issue #166: real, end to end. Splits one HTML spine file into two
//! at a point ([`split`]/[`multisplit`]/[`do_split`]/
//! [`adjust_split_point`], [`SplitLinkReplacer`]), and the inverse:
//! merges multiple spine files into one ([`merge`]/[`merge_html`]/
//! [`merge_css`], [`MergeLinkReplacer`]).
//!
//! # Design notes
//!
//! **Cloning the whole [`Dom`] instead of `getroottree()` + `deepcopy` +
//! re-locating by XPath.** Python's `do_split` computes an XPath `path`
//! for `split_point` *before* deep-copying the tree twice, then
//! re-locates the split point in each copy via `root.xpath(path)[0]`
//! (needed because a Python deep-copy produces new `Element` objects
//! with no relationship to the originals). This crate's [`Dom`] is a
//! plain arena (`Vec<Node>` + integer `NodeId` indices) that now derives
//! `Clone` (see `mobi::dom`'s recent addition); cloning it verbatim
//! preserves every `NodeId`'s meaning unchanged, so the *same*
//! `split_point`/`NodeId` is valid in both the original and every clone
//! -- no XPath round trip needed to re-find it.
//!
//! **`.text`/`.tail` mutation reuses `oeb::polish::pretty`'s helpers.**
//! lxml keeps an element's leading text (`.text`) and following text
//! (`.tail`) as string attributes on the `Element` object itself; `Dom`
//! represents both as ordinary sibling `Text` nodes (see `mobi::dom`'s
//! module docs). `pretty.rs` already implements exactly this
//! text-as-sibling read/write against `Dom` (`leading_text`/
//! `set_leading_text`/`dom_tail`/`set_dom_tail`), so this module reuses
//! those instead of re-deriving them.
//!
//! **No general XPath engine for `loc_or_xpath`'s string form.** Python
//! accepts an arbitrary lxml XPath expression here. Building a general
//! engine is out of scope (`docs/AGENT_PORTING_GUIDE.md`); [`dom_xpath`]
//! instead covers exactly the single-step shape both call sites'
//! documentation promises (`//h:div[@id="split_here"]`) and what
//! [`multisplit`] needs internally (`//*[@calibre-split-point="i"]`),
//! mirroring the same, deliberately-narrower-than-lxml precedent
//! [`super::xmltree::Xml::opf_xpath`] already set for OPF documents.
//!
//! **Cross-file element moves go through [`clone_into`].** `merge_html`
//! copies element subtrees from one file's `Dom` into another's; since
//! each file's `Dom` is its own independent arena (unlike lxml, where
//! any `Element` from any tree can be adopted into any other tree),
//! moving content across files means literally rebuilding the subtree,
//! node by node, in the destination arena.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Result;

use crate::mobi::dom::{Dom, Node as DomNode, NodeId, NodeKind};
use crate::oeb::constants::{OEB_DOCS, OPF2_NS};

use super::container::{href_to_name_at, name_to_href_at, Container, ParsedItem};
use super::errors::PolishError;
use super::pretty::{dom_tail, leading_text, set_dom_tail, set_leading_text};
use super::replace::LinkRebaser;
use super::toc::node_from_loc;

/// Port of `AbortError(ValueError)`: a user-actionable reason a
/// split/merge could not proceed (as opposed to a real bug), matching
/// Python's distinct exception class so callers can tell the two apart.
#[derive(Debug, thiserror::Error)]
pub enum SplitError {
    #[error("{0}")]
    Abort(String),
}

// ===================================================================
// Splitting
// ===================================================================

/// Port of `in_table`.
fn in_table(dom: &Dom, node: NodeId) -> bool {
    let mut cur = Some(node);
    while let Some(n) = cur {
        if dom.tag(n) == Some("table") {
            return true;
        }
        cur = dom.parent(n);
    }
    false
}

/// The element-only index of `id` among its parent's children (lxml's
/// `parent.index(elem)`, which -- unlike this arena's raw
/// `Dom::index_in_parent` -- only counts `Element` children, never the
/// `Text` siblings this arena uses to represent `.text`/`.tail`).
fn element_index(dom: &Dom, id: NodeId) -> Option<usize> {
    let parent = dom.parent(id)?;
    dom.children(parent)
        .into_iter()
        .filter(|&c| matches!(dom.node(c).kind, NodeKind::Element(_)))
        .position(|c| c == id)
}

/// The previous *element* sibling of `id` (lxml's `parent[idx - 1]` when
/// `idx = parent.index(elem)`), skipping over interleaving `Text`
/// siblings.
fn prev_element_sibling(dom: &Dom, id: NodeId) -> Option<NodeId> {
    let parent = dom.parent(id)?;
    let elems: Vec<NodeId> = dom
        .children(parent)
        .into_iter()
        .filter(|&c| matches!(dom.node(c).kind, NodeKind::Element(_)))
        .collect();
    let pos = elems.iter().position(|&c| c == id)?;
    if pos == 0 {
        None
    } else {
        Some(elems[pos - 1])
    }
}

/// Port of `adjust_split_point`: moves the split point up its ancestor
/// chain if it has no content before it. This handles the common case
/// `<div id="chapter1"><h2>Chapter 1</h2>...</div>` with a page break on
/// the `h2`.
pub fn adjust_split_point(dom: &Dom, split_point: NodeId) -> NodeId {
    let mut sp = split_point;
    while let Some(parent) = dom.parent(sp) {
        let parent_tag = dom.tag(parent).unwrap_or("");
        let has_leading_text = leading_text(dom, parent)
            .map(|t| !t.trim().is_empty())
            .unwrap_or(false);
        let idx = element_index(dom, sp).unwrap_or(0);
        if parent_tag == "body" || parent_tag == "html" || has_leading_text || idx > 0 {
            break;
        }
        sp = parent;
    }
    sp
}

fn get_body(dom: &Dom) -> Option<NodeId> {
    dom.find_first_tag_global("body")
}

/// Port of `do_split`'s local `nix_element(elem, top)` helper: removes
/// `id` from its parent (`top = true`, matching `parent.remove(elem)`)
/// or replaces it in place with its own child elements (`top = false`,
/// matching `parent[index:index+1] = list(elem.iterchildren())`).
/// lxml's `.text`/`.tail` live *on* the `Element` object itself, so
/// removing/replacing an element always discards both; ported here as:
/// drop `id`'s own leading text (any `Text` children before its first
/// `Element` child -- lxml's `elem.text`) and drop the `Text` sibling
/// immediately following `id` in its parent (lxml's `elem.tail`),
/// keeping only `id`'s child elements, each still followed by whatever
/// `Text` (its own tail) already came after it -- the arena already
/// represents that as an ordinary sibling, so it travels with the child
/// unchanged.
fn nix_element(dom: &mut Dom, id: NodeId, top: bool) {
    let Some(parent) = dom.parent(id) else {
        return;
    };
    let Some(pos) = dom.index_in_parent(id) else {
        return;
    };
    let siblings = dom.children(parent);
    let tail = siblings
        .get(pos + 1)
        .copied()
        .filter(|&s| matches!(dom.node(s).kind, NodeKind::Text(_)));

    if top {
        dom.detach(id);
        if let Some(t) = tail {
            dom.detach(t);
        }
        return;
    }

    let kids = dom.children(id);
    let first_elem = kids
        .iter()
        .position(|&k| matches!(dom.node(k).kind, NodeKind::Element(_)));
    dom.detach(id);
    if let Some(t) = tail {
        dom.detach(t);
    }
    if let Some(fi) = first_elem {
        for (i, &kid) in kids[fi..].iter().enumerate() {
            dom.insert_child(parent, pos + i, kid);
        }
    }
}

/// Port of `do_split`: splits `dom` into a *before* and an *after* tree
/// at `split_point`. Returns `(before_tree, after_tree)`.
pub fn do_split(dom: &Dom, split_point: NodeId, before: bool) -> (Dom, Dom) {
    // We cannot adjust for `after` since moving an after split point to
    // a parent would break things if the parent contains any content
    // after the original split point.
    let split_point = if before {
        adjust_split_point(dom, split_point)
    } else {
        split_point
    };

    let mut tree = dom.clone();
    let mut tree2 = dom.clone();
    let split_point1 = split_point;
    let split_point2 = split_point;

    let body1 = get_body(&tree).expect("do_split: tree has no <body>");
    let body2 = get_body(&tree2).expect("do_split: tree has no <body>");

    // Tree 1: keep everything up to (and, if `!before`, including) the
    // split point; drop everything from the split point onward.
    let split_point_descendants: HashSet<NodeId> = tree
        .preorder_elements(split_point1)
        .into_iter()
        .filter(|&e| e != split_point1)
        .collect();
    let mut hit_split_point = false;
    let mut keep_descendants = false;
    for elem in tree.preorder_elements(body1) {
        if elem == body1 {
            continue;
        }
        if elem == split_point1 {
            hit_split_point = true;
            if before {
                nix_element(&mut tree, elem, true);
            } else {
                // Keep the split point element (and its descendants) in
                // tree 1; discard its original tail (whatever content
                // followed it is moving to tree 2).
                keep_descendants = true;
                set_dom_tail(&mut tree, elem, "\n");
            }
            continue;
        }
        if hit_split_point {
            if keep_descendants {
                if split_point_descendants.contains(&elem) {
                    continue;
                }
                keep_descendants = false;
            }
            nix_element(&mut tree, elem, true);
        }
    }

    // Tree 2: keep the split point's ancestors (as shells -- their own
    // leading text is cleared since it belongs to content now in tree
    // 1) and, from the split point onward, everything; earlier siblings
    // are unwrapped (their own text is discarded, but their child
    // elements are promoted, since those elements could carry
    // inheritable CSS state via ancestor styling).
    let ancestors: HashSet<NodeId> = {
        let mut anc = HashSet::new();
        let mut cur = tree2.parent(split_point2);
        while let Some(p) = cur {
            anc.insert(p);
            cur = tree2.parent(p);
        }
        anc
    };
    for elem in tree2.preorder_elements(body2) {
        if elem == body2 {
            continue;
        }
        if elem == split_point2 {
            if !before {
                // Keep the split point's tail, if it contains
                // non-whitespace text, by folding it into whatever now
                // immediately precedes it.
                if let Some(tail) = dom_tail(&tree2, elem) {
                    if !tail.trim().is_empty() {
                        if let Some(parent) = tree2.parent(elem) {
                            match prev_element_sibling(&tree2, elem) {
                                None => {
                                    let existing = leading_text(&tree2, parent).unwrap_or_default();
                                    set_leading_text(
                                        &mut tree2,
                                        parent,
                                        &format!("{existing}{tail}"),
                                    );
                                }
                                Some(sib) => {
                                    let existing = dom_tail(&tree2, sib).unwrap_or_default();
                                    set_dom_tail(&mut tree2, sib, &format!("{existing}{tail}"));
                                }
                            }
                        }
                    }
                }
                nix_element(&mut tree2, elem, true);
            }
            break;
        }
        if ancestors.contains(&elem) {
            set_leading_text(&mut tree2, elem, "\n");
        } else {
            nix_element(&mut tree2, elem, false);
        }
    }
    set_leading_text(&mut tree2, body2, "\n");

    (tree, tree2)
}

/// Port of `SplitLinkReplacer`.
pub struct SplitLinkReplacer {
    root: std::path::PathBuf,
    bottom_anchors: HashSet<String>,
    top_name: String,
    bottom_name: String,
    base: String,
    pub replaced: bool,
}

impl SplitLinkReplacer {
    pub fn new(
        container: &Container,
        base: &str,
        bottom_anchors: HashSet<String>,
        top_name: &str,
        bottom_name: &str,
    ) -> Self {
        SplitLinkReplacer {
            root: container.root.clone(),
            bottom_anchors,
            top_name: top_name.to_string(),
            bottom_name: bottom_name.to_string(),
            base: base.to_string(),
            replaced: false,
        }
    }

    /// Port of `SplitLinkReplacer.__call__`.
    pub fn replace(&mut self, url: &str) -> Option<String> {
        if url.is_empty() || url.starts_with('#') {
            return None;
        }
        let name = href_to_name_at(url, &self.root, Some(&self.base))?;
        if name != self.top_name {
            return None;
        }
        let frag = url.split_once('#').map(|(_, f)| f).unwrap_or("");
        if !frag.is_empty() && self.bottom_anchors.contains(frag) {
            self.replaced = true;
            return Some(format!(
                "{}#{frag}",
                name_to_href_at(&self.bottom_name, &self.root, Some(&self.base))
            ));
        }
        None
    }
}

/// All `id`/`name` attribute values anywhere in `dom` (`root.xpath('//*/@id')
/// | root.xpath('//*/@name')`).
fn all_anchors(dom: &Dom) -> HashSet<String> {
    let mut out = HashSet::new();
    for el in dom.preorder_elements(dom.root) {
        if let Some(v) = dom.node(el).attrs.get("id") {
            out.insert(v.clone());
        }
        if let Some(v) = dom.node(el).attrs.get("name") {
            out.insert(v.clone());
        }
    }
    out
}

fn split_suffix_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"_split\d+$").unwrap())
}

fn fix_links_in_split_tree(
    dom: &mut Dom,
    root_path: &Path,
    name: &str,
    bottom_name: &str,
    anchors_in_top: &HashSet<String>,
    anchors_in_bottom: &HashSet<String>,
    is_bottom_tree: bool,
) {
    let hrefs: Vec<NodeId> = dom
        .preorder_elements(dom.root)
        .into_iter()
        .filter(|&e| dom.node(e).attrs.contains_key("href"))
        .collect();
    for a in hrefs {
        let url = dom.node(a).attrs.get("href").cloned().unwrap_or_default();
        let fname = if let Some(frag) = url.strip_prefix('#') {
            let _ = frag;
            name.to_string()
        } else {
            match href_to_name_at(&url, root_path, Some(name)) {
                Some(n) => n,
                None => continue,
            }
        };
        if fname != name {
            continue;
        }
        let frag = url.split_once('#').map(|(_, f)| f).unwrap_or("");
        if anchors_in_top.contains(frag) {
            let new_url = if is_bottom_tree {
                format!(
                    "{}#{frag}",
                    name_to_href_at(name, root_path, Some(bottom_name))
                )
            } else {
                format!("#{frag}")
            };
            dom.node_mut(a).attrs.insert("href".to_string(), new_url);
        } else if anchors_in_bottom.contains(frag) {
            let new_url = if !is_bottom_tree {
                format!(
                    "{}#{frag}",
                    name_to_href_at(bottom_name, root_path, Some(name))
                )
            } else {
                format!("#{frag}")
            };
            dom.node_mut(a).attrs.insert("href".to_string(), new_url);
        }
    }
}

/// Where to split, matching Python's `loc_or_xpath` parameter: either an
/// XPath expression (see [`dom_xpath`]'s docs for the supported subset)
/// or a *loc* (a path of child-element indices from `<body>`, used
/// internally to implement splitting in the preview panel -- see
/// [`super::toc::node_from_loc`]).
pub enum SplitLocation<'a> {
    XPath(&'a str),
    Loc(&'a [usize]),
}

/// Port of `split`: splits the file specified by `name` at the position
/// specified by `loc_or_xpath`, automatically migrating all links and
/// references to the affected files. Returns the name of the newly
/// created (bottom) file.
pub fn split(
    container: &mut Container,
    name: &str,
    loc_or_xpath: SplitLocation<'_>,
    before: bool,
    totals: Option<&[usize]>,
) -> Result<String> {
    container.ensure_parsed(name)?;
    let split_point = {
        let dom = container.get_xhtml(name)?;
        match loc_or_xpath {
            SplitLocation::XPath(expr) => {
                dom_xpath(dom, expr).into_iter().next().ok_or_else(|| {
                    PolishError::MalformedMarkup(format!(
                        "The expression {expr} did not match any nodes"
                    ))
                })?
            }
            SplitLocation::Loc(locs) => node_from_loc(dom, locs, totals)?,
        }
    };

    {
        let dom = container.get_xhtml(name)?;
        if in_table(dom, split_point) {
            return Err(SplitError::Abort("Cannot split inside tables".to_string()).into());
        }
        if dom.tag(split_point) == Some("body") {
            return Err(SplitError::Abort("Cannot split on the <body> tag".to_string()).into());
        }
    }

    let (mut tree1, mut tree2) = {
        let dom = container.get_xhtml(name)?;
        do_split(dom, split_point, before)
    };

    let mut anchors_in_top = all_anchors(&tree1);
    anchors_in_top.insert(String::new());
    let anchors_in_bottom = all_anchors(&tree2);

    let (base, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    let base = split_suffix_re().replace(base, "").to_string();
    let mut nname = String::new();
    let mut suffix = 0u32;
    while nname.is_empty() || container.exists(&nname) {
        suffix += 1;
        nname = format!("{base}_split{suffix}.{ext}");
    }
    let media_type = container
        .base
        .mime_map
        .get(name)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("{name} has no known media type"))?;
    let manifest_item = container.generate_item(&nname, "", Some(&media_type), true)?;
    let opf_name = container.opf_name.clone();
    let manifest_href = container
        .get_xml(&opf_name)?
        .get_attr(manifest_item, "href")
        .unwrap_or("")
        .to_string();
    let bottom_name = container
        .href_to_name(&manifest_href, Some(&opf_name))
        .ok_or_else(|| anyhow::anyhow!("failed to resolve split item name"))?;

    let root_path = container.root.clone();
    fix_links_in_split_tree(
        &mut tree1,
        &root_path,
        name,
        &bottom_name,
        &anchors_in_top,
        &anchors_in_bottom,
        false,
    );
    fix_links_in_split_tree(
        &mut tree2,
        &root_path,
        name,
        &bottom_name,
        &anchors_in_top,
        &anchors_in_bottom,
        true,
    );

    let all_names: Vec<String> = container.base.mime_map.keys().cloned().collect();
    for fname in all_names {
        if fname == name || fname == bottom_name {
            continue;
        }
        let mut repl = SplitLinkReplacer::new(
            container,
            &fname,
            anchors_in_bottom.clone(),
            name,
            &bottom_name,
        );
        container.replace_links(&fname, |url, _ft| repl.replace(url))?;
    }

    container
        .base
        .parsed_cache
        .insert(name.to_string(), ParsedItem::Xhtml(tree1));
    container.dirty(name);
    container
        .base
        .parsed_cache
        .insert(bottom_name.clone(), ParsedItem::Xhtml(tree2));
    container.dirty(&bottom_name);

    let spine_item = container
        .spine_iter()?
        .into_iter()
        .find(|(_, n, _)| n == name)
        .ok_or_else(|| anyhow::anyhow!("{name} is not in the spine"))?;
    let (spine_item_id, _, linear) = spine_item;
    let spine_node = container
        .opf_xpath("//opf:spine")?
        .first()
        .copied()
        .ok_or_else(|| anyhow::anyhow!("OPF has no <spine>"))?;
    let index = {
        let xml = container.get_xml(&opf_name)?;
        xml.element_children(spine_node)
            .iter()
            .position(|&c| c == spine_item_id)
            .map(|p| p + 1)
    }
    .ok_or_else(|| anyhow::anyhow!("spine item not found among spine's children"))?;
    let manifest_id = {
        let xml = container.get_xml(&opf_name)?;
        xml.get_attr(manifest_item, "id").unwrap_or("").to_string()
    };
    {
        let xml = container.get_xml_mut(&opf_name)?;
        let si = xml.new_element("itemref", Some(OPF2_NS));
        xml.set_attr(si, "idref", manifest_id);
        if !linear {
            xml.set_attr(si, "linear", "no");
        }
        xml.insert_element(spine_node, si, Some(index));
    }
    container.dirty(&opf_name);

    Ok(bottom_name)
}

/// Port of `multisplit`: splits `name` at every tag matching `xpath`.
/// Returns the names of every newly created file, in split order.
pub fn multisplit(
    container: &mut Container,
    name: &str,
    xpath: &str,
    before: bool,
) -> Result<Vec<String>> {
    container.ensure_parsed(name)?;
    let nodes: Vec<NodeId> = {
        let dom = container.get_xhtml(name)?;
        dom_xpath(dom, xpath)
    };
    if nodes.is_empty() {
        return Err(
            SplitError::Abort(format!("The expression {xpath} did not match any nodes")).into(),
        );
    }
    {
        let dom = container.get_xhtml(name)?;
        for &n in &nodes {
            if in_table(dom, n) {
                return Err(SplitError::Abort("Cannot split inside tables".to_string()).into());
            }
            if dom.tag(n) == Some("body") {
                return Err(SplitError::Abort("Cannot split on the <body> tag".to_string()).into());
            }
        }
    }
    {
        let dom = container.get_xhtml_mut(name)?;
        for (i, &n) in nodes.iter().enumerate() {
            dom.node_mut(n)
                .attrs
                .insert("calibre-split-point".to_string(), i.to_string());
        }
    }
    container.dirty(name);

    let mut current = name.to_string();
    let mut all_names = vec![name.to_string()];
    for i in 0..nodes.len() {
        let marker = format!(r#"//*[@calibre-split-point="{i}"]"#);
        current = split(
            container,
            &current,
            SplitLocation::XPath(&marker),
            before,
            None,
        )?;
        all_names.push(current.clone());
    }

    for x in &all_names {
        container.ensure_parsed(x)?;
        let marked: Vec<NodeId> = {
            let dom = container.get_xhtml(x)?;
            dom.preorder_elements(dom.root)
                .into_iter()
                .filter(|&e| dom.node(e).attrs.contains_key("calibre-split-point"))
                .collect()
        };
        if !marked.is_empty() {
            let dom = container.get_xhtml_mut(x)?;
            for e in marked {
                dom.node_mut(e).attrs.shift_remove("calibre-split-point");
            }
        }
        container.dirty(x);
    }

    Ok(all_names[1..].to_vec())
}

// ===================================================================
// A narrow XPath-lite for `Dom`, matching what `split`/`multisplit` need.
// ===================================================================

enum DomAttrPredicate<'a> {
    Exists(&'a str),
    Equals(&'a str, &'a str),
}

impl DomAttrPredicate<'_> {
    fn matches(&self, dom: &Dom, id: NodeId) -> bool {
        match self {
            DomAttrPredicate::Exists(a) => dom.node(id).attrs.contains_key(*a),
            DomAttrPredicate::Equals(a, v) => {
                dom.node(id).attrs.get(*a).map(|s| s.as_str()) == Some(*v)
            }
        }
    }
}

/// A narrow XPath-lite for `split`/`multisplit`'s string `loc_or_xpath`
/// form -- see the module docs.
pub fn dom_xpath(dom: &Dom, expr: &str) -> Vec<NodeId> {
    let body = expr.strip_prefix("//").unwrap_or(expr);
    let (tag_part, open) = match body.find('[') {
        Some(open) => (&body[..open], Some(open)),
        None => (body, None),
    };
    let tag = tag_part.split_once(':').map(|(_, t)| t).unwrap_or(tag_part);
    let preds: Vec<DomAttrPredicate<'_>> = match open {
        None => Vec::new(),
        Some(open) => {
            let inner = body[open + 1..].trim_end_matches(']');
            inner
                .split(" and ")
                .filter_map(|p| {
                    let p = p.trim().strip_prefix('@')?;
                    Some(match p.split_once('=') {
                        Some((n, v)) => DomAttrPredicate::Equals(
                            n.trim(),
                            v.trim().trim_matches(|c| c == '"' || c == '\''),
                        ),
                        None => DomAttrPredicate::Exists(p),
                    })
                })
                .collect()
        }
    };
    dom.preorder_elements(dom.root)
        .into_iter()
        .filter(|&id| {
            (tag == "*" || dom.tag(id) == Some(tag)) && preds.iter().all(|p| p.matches(dom, id))
        })
        .collect()
}

// ===================================================================
// Merging
// ===================================================================

/// Deep-clones the subtree rooted at `src_id` (in `src`) into `dst`,
/// returning the id of the new, parentless root node -- the
/// cross-arena equivalent of lxml's `copy.deepcopy(child)` (see the
/// module docs).
fn clone_into(dst: &mut Dom, src: &Dom, src_id: NodeId) -> NodeId {
    match &src.node(src_id).kind {
        NodeKind::Element(tag) => {
            let new_id = dst.new_element(tag);
            for (k, v) in &src.node(src_id).attrs {
                dst.node_mut(new_id).attrs.insert(k.clone(), v.clone());
            }
            let children: Vec<NodeId> = src
                .children(src_id)
                .into_iter()
                .map(|c| clone_into(dst, src, c))
                .collect();
            for c in children {
                dst.append_child(new_id, c);
            }
            new_id
        }
        NodeKind::Text(t) => dst.new_text(t),
        NodeKind::Comment(t) => {
            let id = dst.nodes.len();
            dst.nodes.push(DomNode {
                kind: NodeKind::Comment(t.clone()),
                attrs: indexmap::IndexMap::new(),
                children: Vec::new(),
                parent: None,
            });
            id
        }
        NodeKind::Document => unreachable!("clone_into: cannot clone a Document node"),
    }
}

/// Port of `add_text`.
fn add_text(dom: &mut Dom, body: NodeId, text: &str) {
    if let Some(&last) = dom.children(body).last() {
        if let NodeKind::Text(t) = &mut dom.node_mut(last).kind {
            t.push_str(text);
            return;
        }
    }
    let t = dom.new_text(text);
    dom.append_child(body, t);
}

fn name_dirname(name: &str) -> &str {
    name.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

/// Port of `all_stylesheets`: the names of `name`'s linked (`text/css`)
/// stylesheets.
fn all_stylesheets(container: &mut Container, name: &str) -> Result<Vec<String>> {
    container.ensure_parsed(name)?;
    let dom = container.get_xhtml(name)?;
    let Some(head) = dom.find_first_tag_global("head") else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for c in dom.children(head) {
        if dom.tag(c) != Some("link") {
            continue;
        }
        let Some(href) = dom.node(c).attrs.get("href").cloned() else {
            continue;
        };
        let typ = dom
            .node(c)
            .attrs
            .get("type")
            .cloned()
            .unwrap_or_else(|| "text/css".to_string());
        if typ == "text/css" {
            if let Some(n) = container.href_to_name(&href, Some(name)) {
                out.push(n);
            }
        }
    }
    Ok(out)
}

/// Port of `unique_anchor`.
fn unique_anchor(seen_anchors: &HashSet<String>, current: &str) -> String {
    let mut c = 0u32;
    let mut ans = current.to_string();
    while seen_anchors.contains(&ans) {
        c += 1;
        ans = format!("{current}_{c}");
    }
    ans
}

/// Port of `remove_name_attributes`.
fn remove_name_attributes(dom: &mut Dom) {
    let all = dom.preorder_elements(dom.root);
    for &e in &all {
        if dom.node(e).attrs.contains_key("id") && dom.node(e).attrs.contains_key("name") {
            dom.node_mut(e).attrs.shift_remove("name");
        }
    }
    for &e in &all {
        if let Some(v) = dom.node_mut(e).attrs.shift_remove("name") {
            dom.node_mut(e).attrs.insert("id".to_string(), v);
        }
    }
}

enum MergeChild {
    Text(String),
    Node(NodeId),
}

/// Port of `MergeLinkReplacer`.
pub struct MergeLinkReplacer<'a> {
    root: std::path::PathBuf,
    anchor_map: &'a HashMap<String, HashMap<String, String>>,
    master: String,
    base: String,
    pub replaced: bool,
}

impl<'a> MergeLinkReplacer<'a> {
    pub fn new(
        container: &Container,
        base: &str,
        anchor_map: &'a HashMap<String, HashMap<String, String>>,
        master: &str,
    ) -> Self {
        MergeLinkReplacer {
            root: container.root.clone(),
            anchor_map,
            master: master.to_string(),
            base: base.to_string(),
            replaced: false,
        }
    }

    /// Port of `MergeLinkReplacer.__call__`.
    pub fn replace(&mut self, url: &str) -> Option<String> {
        if url.is_empty() || url.starts_with('#') {
            return None;
        }
        let name = href_to_name_at(url, &self.root, Some(&self.base))?;
        let amap = self.anchor_map.get(&name)?;
        let frag = url.split_once('#').map(|(_, f)| f).unwrap_or("");
        let nfrag = amap.get(frag).map(|s| s.as_str()).unwrap_or(frag);
        self.replaced = true;
        Some(format!(
            "{}#{nfrag}",
            name_to_href_at(&self.master, &self.root, Some(&self.base))
        ))
    }
}

/// Port of `merge_html`: merges the specified HTML `names` into
/// `master`, migrating anchors/links. Returns the id each merged file's
/// first block ended up with in `master` (`first_anchor_map`).
pub fn merge_html(
    container: &mut Container,
    names: &[String],
    master: &str,
    insert_page_breaks: bool,
) -> Result<HashMap<String, String>> {
    container.ensure_parsed(master)?;
    let head_present = container
        .get_xhtml(master)?
        .find_first_tag_global("head")
        .is_some();
    if !head_present {
        let dom = container.get_xhtml_mut(master)?;
        let html = dom
            .find_first_tag_global("html")
            .ok_or_else(|| anyhow::anyhow!("{master} has no <html>"))?;
        let head = dom.new_element("head");
        dom.insert_child(html, 0, head);
        container.dirty(master);
    }

    let mut seen_anchors = all_anchors(container.get_xhtml(master)?);
    let mut seen_stylesheets: HashSet<String> =
        all_stylesheets(container, master)?.into_iter().collect();
    let master_base = name_dirname(master).to_string();

    let mut anchor_map: HashMap<String, HashMap<String, String>> = names
        .iter()
        .filter(|&n| n != master)
        .map(|n| (n.clone(), HashMap::new()))
        .collect();
    let mut first_anchor_map = HashMap::new();

    for name in names {
        if name == master {
            continue;
        }

        for sheet in all_stylesheets(container, name)? {
            if seen_stylesheets.contains(&sheet) {
                continue;
            }
            seen_stylesheets.insert(sheet.clone());
            let href = container.name_to_href(&sheet, Some(master));
            let dom = container.get_xhtml_mut(master)?;
            let head = dom
                .find_first_tag_global("head")
                .ok_or_else(|| anyhow::anyhow!("{master} has no <head>"))?;
            let link = dom.new_element("link");
            dom.node_mut(link)
                .attrs
                .insert("rel".to_string(), "stylesheet".to_string());
            dom.node_mut(link)
                .attrs
                .insert("type".to_string(), "text/css".to_string());
            dom.node_mut(link).attrs.insert("href".to_string(), href);
            dom.append_child(head, link);
            container.dirty(master);
        }

        if name_dirname(name) != master_base {
            let mut repl = LinkRebaser::new(container, name, master);
            container.replace_links(name, |url, _ft| repl.rebase(url))?;
        }

        container.ensure_parsed(name)?;
        let mut source_dom = container.get_xhtml(name)?.clone();

        let bodies = source_dom.find_all_tag_global("body");
        let mut children: Vec<MergeChild> = Vec::new();
        for &body in &bodies {
            let raw = source_dom.children(body);
            let mut idx = 0;
            match raw.first() {
                Some(&first) if matches!(source_dom.node(first).kind, NodeKind::Text(_)) => {
                    let t = match &source_dom.node(first).kind {
                        NodeKind::Text(t) => t.clone(),
                        _ => unreachable!(),
                    };
                    let text = if t.trim().is_empty() {
                        "\n\n".to_string()
                    } else {
                        t
                    };
                    children.push(MergeChild::Text(text));
                    idx = 1;
                }
                _ => children.push(MergeChild::Text("\n\n".to_string())),
            }
            for &c in &raw[idx..] {
                match &source_dom.node(c).kind {
                    NodeKind::Text(t) => children.push(MergeChild::Text(t.clone())),
                    _ => children.push(MergeChild::Node(c)),
                }
            }
        }

        let first_elem_pos = children
            .iter()
            .position(|c| matches!(c, MergeChild::Node(_)));
        let first_child_id: NodeId = if let Some(pos) = first_elem_pos {
            match children[pos] {
                MergeChild::Node(id) => id,
                MergeChild::Text(_) => unreachable!(),
            }
        } else {
            let text = match children.first() {
                Some(MergeChild::Text(t)) => t.clone(),
                _ => String::new(),
            };
            let p = source_dom.new_element("p");
            let t = source_dom.new_text(&text);
            source_dom.append_child(p, t);
            if !children.is_empty() {
                children[0] = MergeChild::Node(p);
            } else {
                children.push(MergeChild::Node(p));
            }
            p
        };

        remove_name_attributes(&mut source_dom);

        {
            let amap = anchor_map
                .get_mut(name)
                .expect("anchor_map has an entry for every non-master name");
            let ids_with_attr: Vec<NodeId> = source_dom
                .preorder_elements(source_dom.root)
                .into_iter()
                .filter(|&e| source_dom.node(e).attrs.contains_key("id"))
                .collect();
            for e in ids_with_attr {
                let val = source_dom
                    .node(e)
                    .attrs
                    .get("id")
                    .cloned()
                    .unwrap_or_default();
                if val.is_empty() {
                    continue;
                }
                if seen_anchors.contains(&val) {
                    let nval = unique_anchor(&seen_anchors, &val);
                    source_dom
                        .node_mut(e)
                        .attrs
                        .insert("id".to_string(), nval.clone());
                    amap.insert(val, nval);
                } else {
                    seen_anchors.insert(val);
                }
            }

            if !source_dom.node(first_child_id).attrs.contains_key("id") {
                let id = unique_anchor(&seen_anchors, "top");
                source_dom
                    .node_mut(first_child_id)
                    .attrs
                    .insert("id".to_string(), id.clone());
                seen_anchors.insert(id);
            }
            let first_id = source_dom
                .node(first_child_id)
                .attrs
                .get("id")
                .cloned()
                .unwrap_or_default();
            first_anchor_map.insert(name.clone(), first_id.clone());

            if insert_page_breaks {
                let existing = source_dom
                    .node(first_child_id)
                    .attrs
                    .get("style")
                    .cloned()
                    .unwrap_or_default();
                source_dom.node_mut(first_child_id).attrs.insert(
                    "style".to_string(),
                    format!("{existing}; page-break-before: always"),
                );
            }

            amap.insert(String::new(), first_id);
        }

        let amap = anchor_map.get(name).cloned().unwrap_or_default();
        let anchor_links: Vec<NodeId> = source_dom
            .preorder_elements(source_dom.root)
            .into_iter()
            .filter(|&e| {
                source_dom.tag(e) == Some("a")
                    && source_dom
                        .node(e)
                        .attrs
                        .get("href")
                        .map(|h| h.starts_with('#'))
                        .unwrap_or(false)
            })
            .collect();
        for a in anchor_links {
            let href = source_dom
                .node(a)
                .attrs
                .get("href")
                .cloned()
                .unwrap_or_default();
            let q = &href[1..];
            if let Some(nq) = amap.get(q) {
                source_dom
                    .node_mut(a)
                    .attrs
                    .insert("href".to_string(), format!("#{nq}"));
            }
        }

        {
            let master_dom = container.get_xhtml_mut(master)?;
            let master_body = *master_dom
                .find_all_tag_global("body")
                .last()
                .ok_or_else(|| anyhow::anyhow!("{master} has no <body>"))?;
            for child in &children {
                match child {
                    MergeChild::Text(t) => add_text(master_dom, master_body, t),
                    MergeChild::Node(id) => {
                        let cloned = clone_into(master_dom, &source_dom, *id);
                        master_dom.append_child(master_body, cloned);
                    }
                }
            }
        }
        container.dirty(master);

        container.remove_item(name, false)?;
    }

    let names_snapshot: Vec<String> = container.base.mime_map.keys().cloned().collect();
    for fname in names_snapshot {
        let mut repl = MergeLinkReplacer::new(container, &fname, &anchor_map, master);
        container.replace_links(&fname, |url, _ft| repl.replace(url))?;
    }

    Ok(first_anchor_map)
}

/// Port of `merge_css`: merges the specified CSS `names` into `master`.
pub fn merge_css(container: &mut Container, names: &[String], master: &str) -> Result<()> {
    let master_base = name_dirname(master).to_string();
    let mut merged = HashSet::new();

    for name in names {
        if name == master {
            continue;
        }
        if name_dirname(name) != master_base {
            let mut repl = LinkRebaser::new(container, name, master);
            container.replace_links(name, |url, _ft| repl.rebase(url))?;
        }

        let sheet = container.parsed_stylesheet(name)?;
        let non_charset: Vec<crate::css::Rule> = sheet
            .rules
            .into_iter()
            .filter(|r| !matches!(r, crate::css::Rule::Charset(_)))
            .collect();
        {
            let mut master_sheet = container.parsed_stylesheet(master)?;
            master_sheet.rules.extend(non_charset);
            let text = master_sheet.to_css_text();
            container.set_css_text(master, text);
        }
        container.dirty(master);

        container.remove_item(name, true)?;
        merged.insert(name.clone());
    }

    let names_snapshot: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(n, mt)| (n.clone(), mt.clone()))
        .collect();
    for (name, mt) in names_snapshot {
        if !OEB_DOCS.contains(&mt.as_str()) {
            continue;
        }
        container.ensure_parsed(&name)?;
        let links: Vec<NodeId> = {
            let dom = container.get_xhtml(&name)?;
            dom.preorder_elements(dom.root)
                .into_iter()
                .filter(|&e| dom.tag(e) == Some("link") && dom.node(e).attrs.contains_key("href"))
                .collect()
        };
        let mut removed = false;
        for link in links {
            let href = {
                let dom = container.get_xhtml(&name)?;
                dom.node(link).attrs.get("href").cloned()
            };
            let Some(href) = href else { continue };
            let Some(q) = container.href_to_name(&href, Some(&name)) else {
                continue;
            };
            if merged.contains(&q) {
                let dom = container.get_xhtml_mut(&name)?;
                dom.detach(link);
                removed = true;
            }
        }
        if removed {
            container.dirty(&name);
            let already_linked = all_stylesheets(container, &name)?
                .iter()
                .any(|s| s == master);
            if !already_linked {
                let href = container.name_to_href(master, Some(&name));
                let dom = container.get_xhtml_mut(&name)?;
                if let Some(head) = dom.find_first_tag_global("head") {
                    let link = dom.new_element("link");
                    dom.node_mut(link)
                        .attrs
                        .insert("type".to_string(), "text/css".to_string());
                    dom.node_mut(link)
                        .attrs
                        .insert("rel".to_string(), "stylesheet".to_string());
                    dom.node_mut(link).attrs.insert("href".to_string(), href);
                    dom.append_child(head, link);
                }
            }
        }
    }

    Ok(())
}

/// Port of `merge`: merges the specified files into a single file,
/// automatically migrating all links and references to the affected
/// files. `category` must be `"text"` (HTML files) or `"styles"` (CSS
/// files); `master` is which of `names` remains after merging.
pub fn merge(
    container: &mut Container,
    category: &str,
    names: &[String],
    master: &str,
) -> Result<()> {
    if category != "text" && category != "styles" {
        return Err(SplitError::Abort(format!("Cannot merge files of type: {category}")).into());
    }
    if names.len() < 2 {
        return Err(
            SplitError::Abort("Must specify at least two files to be merged".to_string()).into(),
        );
    }
    if !names.iter().any(|n| n == master) {
        return Err(SplitError::Abort(format!(
            "The master file ({master}) must be one of the files being merged"
        ))
        .into());
    }

    if category == "text" {
        merge_html(container, names, master, false)?;
    } else {
        merge_css(container, names, master)?;
    }

    container.dirty(master);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_book(dir: &Path) {
        fs::write(
            dir.join("content.opf"),
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata>
    <dc:title>Test Book</dc:title>
    <dc:identifier id="bookid">urn:uuid:12345678-1234-1234-1234-123456789012</dc:identifier>
  </metadata>
  <manifest>
    <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
    <item id="c2" href="chap2.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
    <itemref idref="c2"/>
  </spine>
</package>"#,
        )
        .unwrap();
        fs::write(
            dir.join("chap1.html"),
            b"<html><body><h1 id=\"top\">Chapter One</h1><p>first</p><div id=\"split_here\"><p>second</p></div><p>third</p></body></html>",
        )
        .unwrap();
        fs::write(
            dir.join("chap2.html"),
            b"<html><body><h1>Chapter Two</h1><p>hello</p></body></html>",
        )
        .unwrap();
    }

    #[test]
    fn split_before_produces_two_wellformed_files_whose_content_concatenates() {
        let dir = tempfile::tempdir().unwrap();
        write_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();

        let bottom = split(
            &mut c,
            "chap1.html",
            SplitLocation::XPath(r#"//*[@id="split_here"]"#),
            true,
            None,
        )
        .unwrap();
        assert_eq!(bottom, "chap1_split1.html");

        c.commit(false).unwrap();
        let top_raw = String::from_utf8(fs::read(dir.path().join("chap1.html")).unwrap()).unwrap();
        let bottom_raw = String::from_utf8(fs::read(dir.path().join(&bottom)).unwrap()).unwrap();
        assert!(top_raw.contains("first"));
        assert!(!top_raw.contains("second"));
        assert!(!top_raw.contains("third"));
        assert!(bottom_raw.contains("second"));
        assert!(bottom_raw.contains("third"));
        assert!(!bottom_raw.contains("first"));

        // The new file is in the manifest and spine, right after chap1.html.
        let mut c2 = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let spine_names: Vec<String> = c2
            .spine_names()
            .unwrap()
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(
            spine_names,
            vec![
                "chap1.html".to_string(),
                bottom.clone(),
                "chap2.html".to_string()
            ]
        );
    }

    #[test]
    fn split_after_keeps_split_point_in_top_file() {
        let dir = tempfile::tempdir().unwrap();
        write_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let bottom = split(
            &mut c,
            "chap1.html",
            SplitLocation::XPath(r#"//*[@id="split_here"]"#),
            false,
            None,
        )
        .unwrap();
        c.commit(false).unwrap();
        let top_raw = String::from_utf8(fs::read(dir.path().join("chap1.html")).unwrap()).unwrap();
        let bottom_raw = String::from_utf8(fs::read(dir.path().join(&bottom)).unwrap()).unwrap();
        assert!(top_raw.contains("first"));
        assert!(top_raw.contains("second"));
        assert!(!top_raw.contains("third"));
        assert!(bottom_raw.contains("third"));
        assert!(!bottom_raw.contains("second"));
    }

    #[test]
    fn split_rejects_body_and_table_targets() {
        let dir = tempfile::tempdir().unwrap();
        write_book(dir.path());
        fs::write(
            dir.path().join("chap1.html"),
            b"<html><body><table><tr><td id=\"cell\">x</td></tr></table></body></html>",
        )
        .unwrap();
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let err = split(
            &mut c,
            "chap1.html",
            SplitLocation::XPath(r#"//*[@id="cell"]"#),
            true,
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("table"));
    }

    #[test]
    fn multisplit_splits_at_every_match() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("content.opf"),
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata><dc:title>T</dc:title></metadata>
  <manifest><item id="c1" href="chap1.html" media-type="application/xhtml+xml"/></manifest>
  <spine><itemref idref="c1"/></spine>
</package>"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("chap1.html"),
            b"<html><body><h2 class=\"c\">A</h2><p>1</p><h2 class=\"c\">B</h2><p>2</p><h2 class=\"c\">C</h2><p>3</p></body></html>",
        )
        .unwrap();
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let created = multisplit(&mut c, "chap1.html", r#"//*[@class="c"]"#, true).unwrap();
        // One new file per matched split point (3 `<h2 class="c">`s), on
        // top of the original `chap1.html` that keeps existing.
        assert_eq!(created.len(), 3);
        c.commit(false).unwrap();
        let names = ["chap1.html".to_string()]
            .into_iter()
            .chain(created.iter().cloned())
            .collect::<Vec<_>>();
        let mut all_text = String::new();
        for n in &names {
            all_text.push_str(&String::from_utf8(fs::read(dir.path().join(n)).unwrap()).unwrap());
        }
        assert!(all_text.contains(">A<"));
        assert!(all_text.contains(">B<"));
        assert!(all_text.contains(">C<"));
    }

    #[test]
    fn split_then_merge_round_trips_content() {
        let dir = tempfile::tempdir().unwrap();
        write_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let bottom = split(
            &mut c,
            "chap1.html",
            SplitLocation::XPath(r#"//*[@id="split_here"]"#),
            true,
            None,
        )
        .unwrap();
        merge(
            &mut c,
            "text",
            &["chap1.html".to_string(), bottom.clone()],
            "chap1.html",
        )
        .unwrap();
        c.commit(false).unwrap();
        assert!(!dir.path().join(&bottom).exists());
        let merged = String::from_utf8(fs::read(dir.path().join("chap1.html")).unwrap()).unwrap();
        assert!(merged.contains("first"));
        assert!(merged.contains("second"));
        assert!(merged.contains("third"));

        let mut c2 = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        assert!(!c2.exists(&bottom));
        let spine_names: Vec<String> = c2
            .spine_names()
            .unwrap()
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(
            spine_names,
            vec!["chap1.html".to_string(), "chap2.html".to_string()]
        );
    }

    #[test]
    fn merge_requires_master_to_be_one_of_names() {
        let dir = tempfile::tempdir().unwrap();
        write_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let err = merge(
            &mut c,
            "text",
            &["chap1.html".to_string(), "chap2.html".to_string()],
            "chap3.html",
        )
        .unwrap_err();
        assert!(err.to_string().contains("master file"));
    }

    #[test]
    fn merge_css_combines_stylesheets_and_updates_links() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("content.opf"),
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata><dc:title>T</dc:title></metadata>
  <manifest>
    <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
    <item id="s1" href="a.css" media-type="text/css"/>
    <item id="s2" href="b.css" media-type="text/css"/>
  </manifest>
  <spine><itemref idref="c1"/></spine>
</package>"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("chap1.html"),
            b"<html><head><link rel=\"stylesheet\" type=\"text/css\" href=\"a.css\"/><link rel=\"stylesheet\" type=\"text/css\" href=\"b.css\"/></head><body><p>x</p></body></html>",
        )
        .unwrap();
        fs::write(dir.path().join("a.css"), b"body { color: red; }").unwrap();
        fs::write(dir.path().join("b.css"), b"p { color: blue; }").unwrap();
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        merge(
            &mut c,
            "styles",
            &["a.css".to_string(), "b.css".to_string()],
            "a.css",
        )
        .unwrap();
        c.commit(false).unwrap();
        assert!(!dir.path().join("b.css").exists());
        let merged_css = fs::read_to_string(dir.path().join("a.css")).unwrap();
        assert!(merged_css.contains("color: red"));
        assert!(merged_css.contains("color: blue"));
        let html = fs::read_to_string(dir.path().join("chap1.html")).unwrap();
        assert_eq!(html.matches("a.css").count(), 1);
        assert!(!html.contains("b.css"));
    }
}
