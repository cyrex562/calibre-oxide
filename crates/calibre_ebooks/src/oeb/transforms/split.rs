//! Port of `old_src/src/calibre/ebooks/oeb/transforms/split.py`.
//!
//! Splits over-large or paginated XHTML spine flows into several
//! smaller files: on CSS `page-break-before`/`page-break-after` rules
//! ([`Split::find_page_breaks`]) and, if the output format needs it, to
//! keep every resulting file under `max_flow_size` bytes
//! ([`FlowSplitter::split_to_size`]).
//!
//! # `do_split` is reused directly, not reimplemented
//!
//! Per the batch task notes, [`crate::oeb::polish::split::do_split`] is
//! pure tree-splitting logic over a [`Dom`] -- it takes no `Container` of
//! any kind, so despite living under `oeb::polish`, it applies here
//! unchanged.
//!
//! # No merge functions here
//!
//! Unlike [`crate::oeb::polish::split`] (issue #166), which hosts both a
//! `split`/`multisplit` pair *and* a `merge`/`merge_html`/`merge_css`
//! pair (for the "Polish Book" editor), `old_src`'s
//! `oeb/transforms/split.py` -- verified by reading the actual file, not
//! assumed -- contains no merge logic at all: it only ever *splits* a
//! flow, during format conversion, and never merges spine files back
//! together. There is nothing to port on that side for this file.

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::css::matcher::DomElement;
use crate::css::{selector::parse_selector_list, Select, SelectorList};
use crate::mobi::dom::{Dom, NodeId, NodeKind};
use crate::oeb::book::OEBBook;
use crate::oeb::constants::OEB_STYLES;
use crate::oeb::polish::split::do_split;

const SPLIT_POINT_ATTR: &str = "csp";

/// Port of `SplitError`: raised when no reasonable split point can be
/// found for an over-large flow.
#[derive(Debug, thiserror::Error)]
#[error("Could not find a reasonable point at which to split: {href} sub-tree size: {size_kb} KB")]
pub struct SplitError {
    pub href: String,
    pub size_kb: usize,
}

/// Options this transform reads (`opts.*`/`context.*` in Python).
#[derive(Debug, Clone)]
pub struct SplitOptions {
    pub split_on_page_breaks: bool,
    /// Custom `page-break-before`/`-after` selector override. `None`
    /// means "discover selectors from the book's own stylesheets" (the
    /// common case); `Some` pins a single selector, matching Python's
    /// `page_breaks_xpath` constructor argument (originally an XPath --
    /// narrowed here to a CSS selector, since every real caller of that
    /// argument in the Python codebase passes a plain tag/class
    /// selector, not a general XPath).
    pub page_break_selector: Option<String>,
    /// `0` disables size-based splitting.
    pub max_flow_size: usize,
    pub remove_css_pagebreaks: bool,
    /// The EPUB3 nav document's href, if any -- never split (matches
    /// Python's `is_nav` guard).
    pub epub3_nav_href: Option<String>,
}

impl Default for SplitOptions {
    fn default() -> Self {
        SplitOptions {
            split_on_page_breaks: true,
            page_break_selector: None,
            max_flow_size: 0,
            remove_css_pagebreaks: true,
            epub3_nav_href: None,
        }
    }
}

/// Per-file anchor -> new-file-href map, keyed by the pre-split href.
/// The default (any fragment/no-fragment reference not explicitly
/// recorded) is the first split file, matching Python's
/// `defaultdict(lambda: self.base % 0)`.
struct AnchorMap {
    default: String,
    map: HashMap<String, String>,
}

impl AnchorMap {
    fn get(&self, frag: Option<&str>) -> &str {
        match frag {
            Some(f) => self.map.get(f).map(|s| s.as_str()).unwrap_or(&self.default),
            None => self
                .map
                .get("")
                .map(|s| s.as_str())
                .unwrap_or(&self.default),
        }
    }
}

/// Port of `Split`.
pub struct Split {
    pub opts: SplitOptions,
    /// `item.href -> AnchorMap`, built up as items are split; consulted
    /// by [`Split::fix_links`] at the end.
    map: HashMap<String, AnchorMap>,
}

impl Split {
    pub fn new(opts: SplitOptions) -> Self {
        Split {
            opts,
            map: HashMap::new(),
        }
    }

    /// Port of `Split.__call__`.
    pub fn call(&mut self, oeb: &mut OEBBook, report: &mut dyn FnMut(&str)) -> Result<()> {
        report("Splitting markup on page breaks and flow limits, if any...");
        self.map.clear();

        let selectors = if self.opts.split_on_page_breaks {
            self.find_page_break_selectors(oeb)?
        } else {
            Vec::new()
        };

        let items: Vec<(String, String)> = oeb
            .spine
            .iter()
            .filter_map(|s| {
                oeb.manifest
                    .get_by_id(&s.idref)
                    .map(|i| (s.idref.clone(), i.href.clone()))
            })
            .collect();

        for (idref, href) in items {
            if self.opts.epub3_nav_href.as_deref() == Some(href.as_str()) {
                report(&format!(
                    "Not splitting {href} as it is the EPUB3 nav document"
                ));
                continue;
            }
            self.split_item(oeb, &idref, &href, &selectors)?;
        }

        self.fix_links(oeb);
        Ok(())
    }

    /// Port of `Split.find_page_breaks`: scans every stylesheet's
    /// top-level style rules for `page-break-before`/`page-break-after`
    /// declarations (not walking `@media`/`@import`, a narrower scope
    /// than Python's `css_parser`-backed `rules()` helper -- real-world
    /// page-break rules are essentially always unconditional top-level
    /// rules). When [`SplitOptions::remove_css_pagebreaks`], the
    /// declaration is stripped from the source stylesheet and the file
    /// is written back, matching Python's `rule.style.removeProperty`.
    fn find_page_break_selectors(&self, oeb: &mut OEBBook) -> Result<Vec<(SelectorList, bool)>> {
        if let Some(expr) = &self.opts.page_break_selector {
            let sel = parse_selector_list(expr).map_err(|e| anyhow::anyhow!("{e}"))?;
            return Ok(vec![(sel, true)]);
        }
        let mut out = Vec::new();
        let sheet_hrefs: Vec<String> = oeb
            .manifest
            .iter()
            .filter(|i| OEB_STYLES.contains(&i.media_type.as_str()))
            .map(|i| i.href.clone())
            .collect();
        for href in sheet_hrefs {
            let Ok(data) = oeb.container.read(&href) else {
                continue;
            };
            let text = String::from_utf8_lossy(&data);
            let mut sheet = crate::css::Stylesheet::parse(&text);
            let mut changed = false;
            for rule in sheet.style_rules_mut() {
                let before = rule
                    .style
                    .get_property_value("page-break-before")
                    .trim()
                    .to_lowercase();
                let after = rule
                    .style
                    .get_property_value("page-break-after")
                    .trim()
                    .to_lowercase();
                let ignore = ["avoid", "auto", "inherit", ""];
                if !ignore.contains(&before.as_str()) {
                    if let Ok(sel) = parse_selector_list(&rule.selector_text) {
                        out.push((sel, true));
                    }
                    if self.opts.remove_css_pagebreaks {
                        rule.style.remove_property("page-break-before");
                        changed = true;
                    }
                }
                if !ignore.contains(&after.as_str()) {
                    if let Ok(sel) = parse_selector_list(&rule.selector_text) {
                        out.push((sel, false));
                    }
                    if self.opts.remove_css_pagebreaks {
                        rule.style.remove_property("page-break-after");
                        changed = true;
                    }
                }
            }
            if changed {
                let _ = oeb.container.write(&href, sheet.to_css_text().as_bytes());
            }
        }
        Ok(out)
    }

    fn split_item(
        &mut self,
        oeb: &mut OEBBook,
        idref: &str,
        href: &str,
        selectors: &[(SelectorList, bool)],
    ) -> Result<()> {
        let Ok(raw) = oeb.container.read(href) else {
            return Ok(());
        };
        let html = String::from_utf8_lossy(&raw);
        let mut dom = Dom::parse(&html);

        let page_breaks = if self.opts.split_on_page_breaks {
            find_page_breaks(&mut dom, selectors)
        } else {
            Vec::new()
        };

        let splitter = FlowSplitter::new(dom, href, page_breaks, self.opts.max_flow_size)?;
        if splitter.was_split() {
            let anchor_map = splitter.commit(oeb, idref, href)?;
            self.map.insert(href.to_string(), anchor_map);
        }
        Ok(())
    }

    /// Port of `Split.fix_links`/`rewrite_links`: every remaining
    /// document's internal links that pointed at a now-split file are
    /// redirected to wherever that anchor ended up.
    fn fix_links(&self, oeb: &mut OEBBook) {
        let hrefs: Vec<(String, String)> = oeb
            .manifest
            .iter()
            .map(|i| (i.href.clone(), i.media_type.clone()))
            .collect();
        for (href, media_type) in hrefs {
            if !crate::oeb::constants::OEB_DOCS.contains(&media_type.as_str()) {
                continue;
            }
            let Ok(raw) = oeb.container.read(&href) else {
                continue;
            };
            let text = String::from_utf8_lossy(&raw);
            let mut dom = Dom::parse(&text);
            let mut changed = false;
            let candidates: Vec<NodeId> = dom
                .preorder_elements(dom.root)
                .into_iter()
                .filter(|&e| dom.node(e).attrs.contains_key("href"))
                .collect();
            for elem in candidates {
                let url = dom
                    .node(elem)
                    .attrs
                    .get("href")
                    .cloned()
                    .unwrap_or_default();
                if let Some(new_url) = self.rewrite_link(&href, &url) {
                    if new_url != url {
                        dom.node_mut(elem).attrs.insert("href".to_string(), new_url);
                        changed = true;
                    }
                }
            }
            if changed {
                let rendered = dom.serialize(dom.root).into_bytes();
                let _ = oeb.container.write(&href, &rendered);
            }
        }
    }

    fn rewrite_link(&self, current_href: &str, url: &str) -> Option<String> {
        let (path, frag) = super::filenames::urldefrag(url);
        if path.is_empty() {
            return None;
        }
        let abs = super::filenames::abshref(current_href, &path);
        let abs = super::filenames::urlnormalize(&abs);
        let amap = self.map.get(&abs)?;
        let target = amap.get(if frag.is_empty() {
            None
        } else {
            Some(frag.as_str())
        });
        let mut rel = super::filenames::relhref(current_href, target);
        if !frag.is_empty() {
            rel.push('#');
            rel.push_str(&frag);
        }
        Some(rel)
    }
}

/// One CSS-page-break-triggered split point: which element, and whether
/// the split happens before or after it.
struct PageBreak {
    id: String,
    before: bool,
}

/// Port of `Split.find_page_breaks`'s per-item half: matches
/// `selectors` against `dom`'s `<body>` descendants (excluding
/// structural tags a page-break rule should never apply to), assigns
/// each matched element a stable `id` if it doesn't already have one
/// (writing it into `dom`, matching Python's `x.set('id', ...)`), and
/// returns them in document order.
fn find_page_breaks(dom: &mut Dom, selectors: &[(SelectorList, bool)]) -> Vec<PageBreak> {
    if selectors.is_empty() {
        return Vec::new();
    }
    let Some(body) = dom.find_first_tag_global("body") else {
        return Vec::new();
    };
    let descendants: HashSet<NodeId> = dom
        .preorder_elements(body)
        .into_iter()
        .filter(|&e| e != body)
        .collect();
    let excluded = ["html", "body", "head", "style", "script", "meta", "link"];

    let mut order: HashMap<NodeId, usize> = HashMap::new();
    for (i, e) in dom.preorder_elements(dom.root).into_iter().enumerate() {
        order.insert(e, i);
    }

    let mut matched: HashMap<NodeId, bool> = HashMap::new();
    {
        let elements: Vec<DomElement<'_>> = crate::css::matcher::dom_elements(dom);
        let select = Select::new(elements);
        for (sel, before) in selectors {
            for e in select.matching(sel) {
                if descendants.contains(&e.id) && !excluded.contains(&dom.tag(e.id).unwrap_or("")) {
                    matched.entry(e.id).or_insert(*before);
                }
            }
        }
    }
    let mut ordered: Vec<NodeId> = matched.keys().copied().collect();
    ordered.sort_by_key(|e| order.get(e).copied().unwrap_or(usize::MAX));

    ordered
        .iter()
        .enumerate()
        .map(|(i, &e)| {
            let id = dom.node(e).attrs.get("id").cloned().unwrap_or_else(|| {
                let id = format!("calibre_pb_{i}");
                dom.node_mut(e).attrs.insert("id".to_string(), id.clone());
                id
            });
            PageBreak {
                id,
                before: matched[&e],
            }
        })
        .collect()
}

/// Port of `FlowSplitter`.
struct FlowSplitter {
    trees: Vec<Dom>,
    base_pattern: String,
}

impl FlowSplitter {
    fn new(
        mut dom: Dom,
        href: &str,
        page_breaks: Vec<PageBreak>,
        max_flow_size: usize,
    ) -> Result<Self> {
        let (base, ext) = split_ext(href);
        let base_pattern = format!("{}_split_%03d{}", base.replace('%', "%%"), ext);

        let mut trees = vec![dom.clone()];
        if !page_breaks.is_empty() {
            trees = split_on_page_breaks(&mut dom, &page_breaks);
        }

        if max_flow_size > 0 {
            let mut new_trees = Vec::new();
            for tree in trees {
                let size = tree.serialize(tree.root).len();
                if size > max_flow_size {
                    new_trees.extend(split_to_size(tree, href, max_flow_size)?);
                } else {
                    new_trees.push(tree);
                }
            }
            trees = new_trees;
        }

        Ok(FlowSplitter {
            trees,
            base_pattern,
        })
    }

    fn was_split(&self) -> bool {
        self.trees.len() > 1
    }

    /// Port of `FlowSplitter.commit`: assigns each surviving tree a
    /// manifest item, splices them into the spine in place of the
    /// original, and returns the per-anchor map used to fix up links
    /// elsewhere in the book.
    fn commit(&self, oeb: &mut OEBBook, idref: &str, orig_href: &str) -> Result<AnchorMap> {
        let files: Vec<String> = (0..self.trees.len())
            .map(|i| self.base_pattern.replacen("%03d", &format!("{i:03}"), 1))
            .collect();
        let default_file = files[0].clone();
        let mut anchors: HashMap<String, String> = HashMap::new();
        for (tree, file) in self.trees.iter().zip(&files) {
            for e in tree.preorder_elements(tree.root) {
                for key in ["id", "name"] {
                    if let Some(v) = tree.node(e).attrs.get(key) {
                        if !v.is_empty() {
                            anchors.entry(v.clone()).or_insert_with(|| file.clone());
                        }
                    }
                }
            }
        }

        let media_type = oeb
            .manifest
            .get_by_href(orig_href)
            .map(|i| i.media_type.clone())
            .unwrap_or_else(|| "application/xhtml+xml".to_string());
        let linear = oeb
            .spine
            .index_of(idref)
            .and_then(|i| oeb.spine.items.get(i))
            .map(|s| s.linear)
            .unwrap_or(true);
        let spine_pos = oeb.spine.index_of(idref).unwrap_or(oeb.spine.items.len());

        let mut new_idrefs = Vec::new();
        for (tree, file) in self.trees.iter().zip(&files) {
            let mut t = tree.clone();
            // Fix intra-flow `#fragment` links so a link that used to
            // resolve within the single original file still resolves
            // once its target has moved to a different split file.
            let anchor_links: Vec<NodeId> = t
                .preorder_elements(t.root)
                .into_iter()
                .filter(|&e| {
                    t.tag(e) == Some("a")
                        && t.node(e)
                            .attrs
                            .get("href")
                            .map(|h| h.starts_with('#'))
                            .unwrap_or(false)
                })
                .collect();
            for a in anchor_links {
                let href = t.node(a).attrs.get("href").cloned().unwrap_or_default();
                let target = anchors
                    .get(&href[1..])
                    .cloned()
                    .unwrap_or_else(|| default_file.clone());
                if &target != file {
                    let new_href =
                        format!("{}{}", super::filenames::relhref(orig_href, &target), href);
                    t.node_mut(a).attrs.insert("href".to_string(), new_href);
                }
            }

            let rendered = t.serialize(t.root).into_bytes();
            let (id, href) = oeb.manifest.generate(idref, file);
            oeb.manifest.add(&id, &href, &media_type);
            let _ = oeb.container.write(&href, &rendered);
            new_idrefs.push(id);
        }

        oeb.spine.remove_by_idref(idref);
        for (i, id) in new_idrefs.into_iter().enumerate() {
            oeb.spine.insert(spine_pos + i, &id, linear);
        }
        oeb.manifest.remove(idref);

        // Guide/TOC/pages hrefs pointing into `orig_href` now resolve
        // through the anchor map too.
        let guide_types: Vec<String> = oeb.guide.types().cloned().collect();
        for t in guide_types {
            if let Some(r) = oeb.guide.references.get(&t) {
                let (path, frag) = super::filenames::urldefrag(&r.href);
                if path == orig_href {
                    let target = anchors
                        .get(&frag)
                        .cloned()
                        .unwrap_or_else(|| default_file.clone());
                    let mut nhref = target;
                    if !frag.is_empty() {
                        nhref.push('#');
                        nhref.push_str(&frag);
                    }
                    if let Some(r) = oeb.guide.references.get_mut(&t) {
                        r.href = nhref;
                    }
                }
            }
        }
        fix_toc_entry(&mut oeb.toc.root, orig_href, &anchors, &default_file);
        for page in &mut oeb.pages.pages {
            let (path, frag) = super::filenames::urldefrag(&page.href);
            if path == orig_href {
                let target = anchors
                    .get(&frag)
                    .cloned()
                    .unwrap_or_else(|| default_file.clone());
                let mut nhref = target;
                if !frag.is_empty() {
                    nhref.push('#');
                    nhref.push_str(&frag);
                }
                page.href = nhref;
            }
        }

        Ok(AnchorMap {
            default: default_file,
            map: anchors,
        })
    }
}

fn fix_toc_entry(
    node: &mut crate::oeb::toc::TOCNode,
    orig_href: &str,
    anchors: &HashMap<String, String>,
    default_file: &str,
) {
    if let Some(href) = &node.href {
        let (path, frag) = super::filenames::urldefrag(href);
        if path == orig_href {
            let target = anchors
                .get(&frag)
                .cloned()
                .unwrap_or_else(|| default_file.to_string());
            let mut nhref = target;
            if !frag.is_empty() {
                nhref.push('#');
                nhref.push_str(&frag);
            }
            node.href = Some(nhref);
        }
    }
    for c in &mut node.children {
        fix_toc_entry(c, orig_href, anchors, default_file);
    }
}

fn split_ext(href: &str) -> (String, String) {
    let slash = href.rfind('/').map(|i| i + 1).unwrap_or(0);
    let base = &href[slash..];
    match base.rfind('.') {
        Some(i) if i > 0 => (href[..slash + i].to_string(), base[i..].to_string()),
        _ => (href.to_string(), String::new()),
    }
}

/// Port of `FlowSplitter.split_on_page_breaks`: repeatedly finds the
/// first remaining page-break id (in document order), locates it in
/// whichever of the current trees still contains it (searched most-recent
/// first, matching Python's `for i in range(len(self.trees)-1, -1, -1)`),
/// and splits that tree there. Trees left with no real content are
/// dropped, carrying their non-generated ids forward as zero-height
/// anchor `<div>`s at the top of the next surviving tree's body (so
/// internal links to those anchors still resolve).
fn split_on_page_breaks(orig: &mut Dom, page_breaks: &[PageBreak]) -> Vec<Dom> {
    let mut trees = vec![orig.clone()];
    for pb in page_breaks {
        for i in (0..trees.len()).rev() {
            if let Some(elem) = trees[i].find_by_id(&pb.id) {
                let (before_tree, after_tree) = do_split(&trees[i], elem, pb.before);
                trees.splice(i..=i, [before_tree, after_tree]);
                break;
            }
        }
    }

    let mut out = Vec::new();
    let mut pending_ids: Vec<String> = Vec::new();
    for tree in trees {
        if is_page_empty(&tree) {
            let Some(body) = tree.find_first_tag_global("body") else {
                out.push(tree);
                continue;
            };
            for e in tree.preorder_elements(body) {
                if let Some(id) = tree.node(e).attrs.get("id") {
                    if !id.starts_with("calibre_") {
                        pending_ids.push(id.clone());
                    }
                }
            }
        } else {
            let mut t = tree;
            if !pending_ids.is_empty() {
                if let Some(body) = t.find_first_tag_global("body") {
                    let existing: HashSet<String> = t
                        .preorder_elements(body)
                        .into_iter()
                        .filter_map(|e| t.node(e).attrs.get("id").cloned())
                        .collect();
                    for (offset, id) in pending_ids
                        .iter()
                        .filter(|id| !existing.contains(*id))
                        .enumerate()
                    {
                        let div = t.new_element("div");
                        t.node_mut(div).attrs.insert("id".to_string(), id.clone());
                        t.node_mut(div)
                            .attrs
                            .insert("style".to_string(), "height:0pt".to_string());
                        t.insert_child(body, offset, div);
                    }
                }
                pending_ids.clear();
            }
            out.push(t);
        }
    }
    out
}

/// Port of `FlowSplitter.is_page_empty`.
fn is_page_empty(tree: &Dom) -> bool {
    let Some(body) = tree.find_first_tag_global("body") else {
        return false;
    };
    let text = tree.text_content(body);
    let stripped: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\u{a0}')
        .collect();
    if !stripped.is_empty() {
        return false;
    }
    for img in tree.find_all_tag_global("img") {
        if tree.node(img).attrs.get("style").map(|s| s.as_str()) != Some("display:none") {
            return false;
        }
    }
    if !tree.find_all_tag_global("svg").is_empty() {
        return false;
    }
    true
}

const SPLIT_PRIORITY: &[&[&str]] = &[
    &["h1", "h2", "h3", "h4", "h5", "h6"],
    &["div"],
    &["pre"],
    &["hr"],
    &["p"],
    &["div"],
    &["br"],
    &["li"],
];

/// Port of `FlowSplitter.find_split_point`'s `pick_elem`: the element in
/// the "middle" of `elems` that hasn't already been tried (marked with
/// [`SPLIT_POINT_ATTR`]), if any -- and marks it tried.
fn pick_elem(dom: &mut Dom, elems: &[NodeId]) -> Option<NodeId> {
    let untried: Vec<NodeId> = elems
        .iter()
        .copied()
        .filter(|&e| dom.node(e).attrs.get(SPLIT_POINT_ATTR).map(|s| s.as_str()) != Some("1"))
        .collect();
    if untried.is_empty() {
        return None;
    }
    let pick = untried[untried.len() / 2];
    dom.node_mut(pick)
        .attrs
        .insert(SPLIT_POINT_ATTR.to_string(), "1".to_string());
    Some(pick)
}

/// Port of `FlowSplitter.find_split_point`.
fn find_split_point(dom: &mut Dom) -> Option<NodeId> {
    for (i, tags) in SPLIT_PRIORITY.iter().enumerate() {
        let elems: Vec<NodeId> = if i == 1 {
            // `/h:html/h:body/h:div`: direct children of body only.
            dom.find_first_tag_global("body")
                .map(|b| dom.children(b))
                .unwrap_or_default()
                .into_iter()
                .filter(|&c| dom.tag(c) == Some("div"))
                .collect()
        } else {
            tags.iter()
                .flat_map(|t| dom.find_all_tag_global(t))
                .collect()
        };
        if let Some(e) = pick_elem(dom, &elems) {
            return Some(e);
        }
    }
    None
}

/// Port of `FlowSplitter.split_text`: splits `text` on blank lines into
/// chunks each under `size` bytes (used only for oversized `<pre>`
/// blocks).
fn split_pre_text(text: &str, size: usize) -> Result<Vec<String>> {
    let rest = text.replace('\r', "");
    let parts: Vec<&str> = rest.split("\n\n").collect();
    if parts.iter().any(|p| p.len() > size) {
        return Err(anyhow::anyhow!(
            "Cannot split as file contains a <pre> tag with a very large paragraph"
        ));
    }
    let mut out = Vec::new();
    let mut buf = String::new();
    for part in parts {
        if buf.len() + part.len() < size {
            buf.push_str("\n\n");
            buf.push_str(part);
        } else {
            out.push(buf);
            buf = part.to_string();
        }
    }
    if !buf.is_empty() || out.is_empty() {
        out.push(buf);
    }
    Ok(out)
}

/// Port of `FlowSplitter.split_to_size`, recursive: splits `tree` at
/// [`find_split_point`], keeping halves under `max_flow_size` and
/// recursing on any half still too large. Errors with [`SplitError`] if
/// no split point can be found at all.
fn split_to_size(mut tree: Dom, href: &str, max_flow_size: usize) -> Result<Vec<Dom>> {
    // Pre-split any oversized single-paragraph <pre> block, matching
    // Python's dedicated pass before the general split-point search.
    let pres: Vec<NodeId> = tree.find_all_tag_global("pre");
    for pre in pres {
        let has_element_children = tree
            .children(pre)
            .iter()
            .any(|&c| matches!(tree.node(c).kind, NodeKind::Element(_)));
        if has_element_children {
            continue;
        }
        let text = tree.text_content(pre);
        if text.len() as f64 > max_flow_size as f64 * 0.5 {
            if let Ok(frags) = split_pre_text(&text, (max_flow_size as f64 * 0.2) as usize) {
                if frags.len() > 1 {
                    let Some(parent) = tree.parent(pre) else {
                        continue;
                    };
                    let Some(idx) = tree.index_in_parent(pre) else {
                        continue;
                    };
                    tree.detach(pre);
                    for (i, frag) in frags.into_iter().enumerate() {
                        let p2 = tree.new_element("pre");
                        let t = tree.new_text(&frag);
                        tree.append_child(p2, t);
                        tree.insert_child(parent, idx + i, p2);
                    }
                }
            }
        }
    }

    let Some(split_point) = find_split_point(&mut tree) else {
        return Err(SplitError {
            href: href.to_string(),
            size_kb: tree.serialize(tree.root).len() / 1024,
        }
        .into());
    };

    let (t1, t2) = do_split(&tree, split_point, true);
    let s1 = t1.serialize(t1.root).len();
    let s2 = t2.serialize(t2.root).len();
    if s1.min(s2) < 5 * 1024 {
        // Split tree too small -- try again from a different point in the
        // *original* tree (which still carries every previously-tried
        // split-point marker).
        return split_to_size(tree, href, max_flow_size);
    }

    let mut out = Vec::new();
    for (t, size) in [(t1, s1), (t2, s2)] {
        if is_page_empty(&t) {
            continue;
        } else if size <= max_flow_size {
            out.push(t);
        } else {
            out.extend(split_to_size(t, href, max_flow_size)?);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::transforms::test_support::Builder;

    #[test]
    fn splits_on_a_page_break_before_css_rule() {
        let mut oeb = Builder::new()
            .part(
                "style.css",
                "text/css",
                b"h2 { page-break-before: always }",
                false,
            )
            .part(
                "a.html",
                "application/xhtml+xml",
                br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><link rel="stylesheet" href="style.css"/></head><body><p>intro</p><h2>Two</h2><p>more</p></body></html>"#,
                true,
            )
            .build();
        let mut splitter = Split::new(SplitOptions::default());
        let mut log = Vec::new();
        splitter
            .call(&mut oeb, &mut |m| log.push(m.to_string()))
            .unwrap();

        let names: HashSet<String> = oeb.manifest.iter().map(|i| i.href.clone()).collect();
        assert!(names.len() >= 2, "{names:?}");
        let mut all_text = String::new();
        for name in &names {
            if name.ends_with(".html") {
                let raw = oeb.container.read(name).unwrap();
                all_text.push_str(&String::from_utf8_lossy(&raw));
            }
        }
        assert!(all_text.contains("intro"));
        assert!(all_text.contains("Two"));
        assert!(all_text.contains("more"));
        // The page-break declaration should have been consumed (removed
        // from the source stylesheet) by default.
        let css = oeb.container.read("style.css").unwrap();
        assert!(!String::from_utf8_lossy(&css).contains("page-break-before"));
    }

    #[test]
    fn no_page_breaks_leaves_the_item_unsplit() {
        let mut oeb = Builder::new()
            .page("a.html", "<p>only one page</p>")
            .build();
        let mut splitter = Split::new(SplitOptions::default());
        let mut log = Vec::new();
        splitter
            .call(&mut oeb, &mut |m| log.push(m.to_string()))
            .unwrap();
        assert_eq!(oeb.manifest.items.len(), 1);
        assert_eq!(oeb.spine.items.len(), 1);
    }

    #[test]
    fn split_updates_toc_href_to_the_file_containing_the_anchor() {
        let mut oeb = Builder::new()
            .part(
                "style.css",
                "text/css",
                b"h2 { page-break-before: always }",
                false,
            )
            .part(
                "a.html",
                "application/xhtml+xml",
                br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><link rel="stylesheet" href="style.css"/></head><body><p>intro</p><h2 id="ch2">Two</h2><p>more</p></body></html>"#,
                true,
            )
            .build();
        oeb.toc.root.add(crate::oeb::toc::TOCNode::new(
            Some("Chapter 2".to_string()),
            Some("a.html#ch2".to_string()),
        ));
        let mut splitter = Split::new(SplitOptions::default());
        let mut log = Vec::new();
        splitter
            .call(&mut oeb, &mut |m| log.push(m.to_string()))
            .unwrap();
        let new_href = oeb.toc.root.children[0].href.clone().unwrap();
        assert_ne!(new_href, "a.html#ch2");
        assert!(new_href.ends_with("#ch2"), "{new_href}");
    }

    #[test]
    fn find_split_point_prefers_headings() {
        let mut dom =
            Dom::parse("<html><body><p>a</p><h2>b</h2><p>c</p><h2>d</h2><p>e</p></body></html>");
        let sp = find_split_point(&mut dom).unwrap();
        assert_eq!(dom.tag(sp), Some("h2"));
    }
}
