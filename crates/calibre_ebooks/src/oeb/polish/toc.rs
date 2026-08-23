//! Port of `old_src/src/calibre/ebooks/oeb/polish/toc.py`.
//!
//! This is calibre's TOC (table of contents) editor: parsing an existing
//! NCX (EPUB2) or nav-document (EPUB3) TOC into an in-memory [`Toc`] tree,
//! building one from OPF spine hrefs/HTML headings/links when none
//! exists, generating landmarks, and committing edits back as NCX and/or
//! nav XHTML.
//!
//! # The [`Toc`] tree
//!
//! Python's `TOC` class is a self-referential tree: each node is its own
//! Python object, holding a `parent` back-reference and a `children`
//! list of more `TOC` objects. Rust has no straightforward equivalent of
//! that shape without `Rc<RefCell<_>>` boilerplate throughout, so -- like
//! [`crate::xmltree::Xml`] and [`crate::dom::Dom`] before it -- this
//! is ported as an **arena**: [`Toc`] owns a flat `Vec<TocNode>`, and
//! every node is referred to by a small `Copy` handle, [`TocNodeId`].
//! [`TocNode`]'s fields mirror `TOC`'s instance attributes 1:1
//! (`title`/`dest`/`frag`/`dest_exists`/`dest_error`/`children`/`parent`);
//! tree mutation (`add`/`remove`/`remove_from_parent`) are [`Toc`]
//! methods taking a `TocNodeId` rather than methods on a self-contained
//! node object, the same shape `Xml::insert_element`/`Xml::detach` use.
//!
//! **This is a different, unrelated type from
//! [`crate::metadata::toc::TOC`]`/`[`crate::metadata::toc::TOCNode`]`,**
//! which is a much simpler recursive `title`+`src`+`children` tree used
//! by the MOBI reader/writer pipeline (issues #33-#35) and has no
//! `play_order`/`id`/`klass`/`frag`/dest-verification/landmark concerns.
//! Do not conflate the two.
//!
//! `TOC.to_dict`/`.as_dict` (issue: JSON serialization for calibre's
//! browser-based book viewer, `srv/render_book.py`) is **not** ported:
//! nothing else in this file depends on it, and its only consumer is an
//! HTTP server component (`calibre.srv`) that is entirely out of scope
//! for this port. Every other public method/property on Python's `TOC`
//! class is ported for real.
//!
//! # `_('...')`-translated strings
//!
//! `calibre.translations.dynamic.translate` (calibre's full runtime i18n
//! system) is out of scope for this port, matching every other file
//! ported so far -- the literal English source string is used as an
//! identity passthrough (see [`create_inline_toc`]'s doc comment for the
//! one call site in this file that used it).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::OnceLock;

use anyhow::Result;
use regex::Regex;

use crate::dom::{Dom, NodeId, NodeKind};
use crate::oeb::constants::OEB_DOCS;

use super::container::{name_to_href_at, Container, ParsedItem};
use super::errors::PolishError;
use super::pretty::{pretty_dom_xml_tree, pretty_html_tree};
use super::utils::guess_type;
use crate::xmltree::{Xml, XmlNodeId};

fn whitespace_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\s+").unwrap())
}

/// `urlparse(href).fragment or None`: the part of `href` after the first
/// `#`, or `None` if there is none or it is empty.
fn url_fragment(href: &str) -> Option<String> {
    href.split_once('#')
        .map(|(_, f)| f.to_string())
        .filter(|f| !f.is_empty())
}

// ===================================================================
// The TOC tree
// ===================================================================

pub type TocNodeId = usize;

/// One node of a [`Toc`] tree. Port of a `TOC` instance's attributes.
#[derive(Debug, Clone, Default)]
pub struct TocNode {
    pub title: Option<String>,
    pub dest: Option<String>,
    pub frag: Option<String>,
    pub dest_exists: Option<bool>,
    pub dest_error: Option<String>,
    pub children: Vec<TocNodeId>,
    pub parent: Option<TocNodeId>,
}

/// A single `<pagetarget>`/page-list `<li>` entry. Port of the `dict`
/// Python appends to `TOC.page_list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageListEntry {
    pub dest: Option<String>,
    pub pagenum: String,
    pub frag: Option<String>,
}

/// A single guide-reference/nav-landmark entry. Port of the `dict`
/// Python yields from `get_guide_landmarks`/`get_nav_landmarks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landmark {
    pub dest: String,
    pub frag: String,
    pub title: String,
    pub r#type: String,
}

/// Port of the `TOC` class. See the module docs for the arena design and
/// why this is unrelated to [`crate::metadata::toc::TOC`].
#[derive(Debug, Clone)]
pub struct Toc {
    nodes: Vec<TocNode>,
    /// The arena's root pseudo-node -- corresponds to the top-level `TOC()`
    /// instance every ported function builds/receives; its `title`/`dest`
    /// are always `None`; real entries are its descendants.
    pub root: TocNodeId,
    /// Set on the root only, matching Python's `toc_root.lang = ...`
    /// idiom (attributes assigned ad hoc after construction, only ever
    /// on the object callers treat as "the root").
    pub lang: Option<String>,
    pub uid: Option<String>,
    pub toc_title: Option<String>,
    pub toc_file_name: Option<String>,
    pub page_list: Vec<PageListEntry>,
}

impl Default for Toc {
    fn default() -> Self {
        Self::new()
    }
}

impl Toc {
    /// Port of `TOC.__init__` for a fresh, empty `TOC()`.
    pub fn new() -> Self {
        Toc {
            nodes: vec![TocNode::default()],
            root: 0,
            lang: None,
            uid: None,
            toc_title: None,
            toc_file_name: None,
            page_list: Vec::new(),
        }
    }

    pub fn node(&self, id: TocNodeId) -> &TocNode {
        &self.nodes[id]
    }

    pub fn node_mut(&mut self, id: TocNodeId) -> &mut TocNode {
        &mut self.nodes[id]
    }

    pub fn children(&self, id: TocNodeId) -> &[TocNodeId] {
        &self.nodes[id].children
    }

    pub fn parent(&self, id: TocNodeId) -> Option<TocNodeId> {
        self.nodes[id].parent
    }

    /// Port of `TOC.__len__`: the number of direct children.
    pub fn len(&self, id: TocNodeId) -> usize {
        self.nodes[id].children.len()
    }

    pub fn is_empty(&self, id: TocNodeId) -> bool {
        self.nodes[id].children.is_empty()
    }

    /// Port of `TOC.add`.
    pub fn add(
        &mut self,
        parent: TocNodeId,
        title: Option<String>,
        dest: Option<String>,
        frag: Option<String>,
    ) -> TocNodeId {
        // `if self.title: self.title = self.title.strip()` -- only a
        // truthy (non-empty) title gets stripped, but stripping an
        // already-empty string is a no-op, so this covers both cases.
        let title = title.map(|t| t.trim().to_string());
        let id = self.nodes.len();
        self.nodes.push(TocNode {
            title,
            dest,
            frag,
            dest_exists: None,
            dest_error: None,
            children: Vec::new(),
            parent: Some(parent),
        });
        self.nodes[parent].children.push(id);
        id
    }

    /// Port of `TOC.remove`.
    pub fn remove(&mut self, parent: TocNodeId, child: TocNodeId) {
        self.nodes[parent].children.retain(|&c| c != child);
        self.nodes[child].parent = None;
    }

    /// Port of `TOC.remove_from_parent`: detaches `id`, promoting its
    /// own children into its former position among its parent's
    /// children (in their original order -- see the inline note in
    /// `old_src` for why repeated `insert(idx, ...)` in reverse order
    /// produces that; this achieves the identical final ordering more
    /// directly).
    pub fn remove_from_parent(&mut self, id: TocNodeId) {
        let Some(parent) = self.nodes[id].parent else {
            return;
        };
        let Some(idx) = self.nodes[parent].children.iter().position(|&c| c == id) else {
            return;
        };
        self.nodes[parent].children.remove(idx);
        let kids = std::mem::take(&mut self.nodes[id].children);
        for (offset, &kid) in kids.iter().enumerate() {
            self.nodes[kid].parent = Some(parent);
            self.nodes[parent].children.insert(idx + offset, kid);
        }
        self.nodes[id].parent = None;
    }

    /// Port of `TOC.iterdescendants` (the `level=None` form -- every
    /// caller in this file uses that form).
    pub fn iterdescendants(&self, id: TocNodeId) -> Vec<TocNodeId> {
        let mut out = Vec::new();
        self.collect_descendants(id, &mut out);
        out
    }

    fn collect_descendants(&self, id: TocNodeId, out: &mut Vec<TocNodeId>) {
        for &c in &self.nodes[id].children {
            out.push(c);
            self.collect_descendants(c, out);
        }
    }

    /// Port of `TOC.remove_duplicates`.
    pub fn remove_duplicates(&mut self, id: TocNodeId, only_text: bool) {
        type Key = (Option<String>, Option<String>, Option<String>);
        let mut seen: HashSet<Key> = HashSet::new();
        let mut remove = Vec::new();
        for &child in self.nodes[id].children.clone().iter() {
            let key: Key = if only_text {
                (self.nodes[child].title.clone(), None, None)
            } else {
                (
                    self.nodes[child].title.clone(),
                    self.nodes[child].dest.clone(),
                    self.nodes[child].frag.clone(),
                )
            };
            if seen.contains(&key) {
                remove.push(child);
            } else {
                seen.insert(key);
                self.remove_duplicates(child, only_text);
            }
        }
        for child in remove {
            self.remove(id, child);
        }
    }

    /// Port of the `TOC.depth` property.
    pub fn depth(&self, id: TocNodeId) -> usize {
        let children = &self.nodes[id].children;
        if children.is_empty() {
            1
        } else {
            1 + children.iter().map(|&c| self.depth(c)).max().unwrap_or(0)
        }
    }

    /// Port of the `TOC.last_child` property.
    pub fn last_child(&self, id: TocNodeId) -> Option<TocNodeId> {
        self.nodes[id].children.last().copied()
    }

    /// Port of `TOC.get_lines`.
    pub fn get_lines(&self, id: TocNodeId, lvl: usize) -> Vec<String> {
        let node = &self.nodes[id];
        let frag = node
            .frag
            .as_deref()
            .map(|f| format!("#{f}"))
            .unwrap_or_default();
        let title = node.title.as_deref().unwrap_or("None");
        let dest = node.dest.as_deref().unwrap_or("None");
        let mut out = vec![format!("{}TOC: {title} --> {dest}{frag}", "\t".repeat(lvl))];
        for &c in &self.nodes[id].children {
            out.extend(self.get_lines(c, lvl + 1));
        }
        out
    }
}

impl std::fmt::Display for Toc {
    /// Port of `TOC.__str__`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.get_lines(self.root, 0).join("\n"))
    }
}

// ===================================================================
// Localization stand-ins
// ===================================================================

/// Narrow stand-in for `calibre.utils.localization.get_lang` (calibre's
/// current UI-locale language code). No locale/preference subsystem
/// exists in this port, so this always returns the same ISO-639-2
/// fallback `sanitize_lang`/`get_lang` land on when nothing else is
/// configured.
fn get_lang() -> String {
    "eng".to_string()
}

/// Narrow stand-in for `calibre.utils.localization.canonicalize_lang`.
/// Two independent, equally-narrow copies of this already exist at
/// `crate::mobi::headers::canonicalize_lang` and
/// `super::opf::canonicalize_lang` (see the latter's doc comment for why
/// each call site gets its own copy rather than sharing one): real
/// ISO-639 table lookups are out of scope, every call site here only
/// needs "best effort" lowercase+trim normalization.
fn canonicalize_lang(raw: &str) -> Option<String> {
    let s = raw.trim().to_lowercase();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Narrow stand-in for `calibre.utils.localization.lang_as_iso639_1`.
/// Real conversion needs the same ISO-639 table `canonicalize_lang`
/// doesn't have. This only handles the extremely common case of a code
/// whose first `-`/`_`-separated subtag is already 2 ASCII letters
/// (`"en"` -> `"en"`, `"en-GB"` -> `"en"`); anything else (a bare
/// 3-letter ISO-639-2/3 code needing table lookup, e.g. `"eng"`) returns
/// `None`, matching Python's `lang_as_iso639_1(x) or x`
/// fallback-to-original pattern used at every call site in this file.
fn lang_as_iso639_1(lang: &str) -> Option<String> {
    let first = lang.split(['-', '_']).next().unwrap_or(lang);
    if first.len() == 2 && first.chars().all(|c| c.is_ascii_alphabetic()) {
        Some(first.to_lowercase())
    } else {
        None
    }
}

/// Narrow stand-in for `calibre.ebooks.oeb.base.uuid_id`
/// (`'u' + short_uuid.uuid4()`, calibre's own base-57-ish short-UUID
/// encoding). No `short_uuid` port exists in this crate; a standard
/// UUIDv4 (already a workspace dependency, see `container.rs`'s
/// `uuid::Uuid` usage) rendered without hyphens produces an equally
/// unique, equally valid XML NCName (starts with the required `u`
/// letter) -- just via a different, simpler encoding. `pub(crate)`:
/// `oeb::polish::cover`'s `create_epub_cover` (`cover.py`'s own `from
/// calibre.ebooks.oeb.base import uuid_id`) needs the same helper.
pub(crate) fn uuid_id() -> String {
    format!("u{}", uuid::Uuid::new_v4().simple())
}

// ===================================================================
// NCX parsing
// ===================================================================

/// Case-insensitive direct-element-children lookup, matching
/// `child_xpath`'s `calibre:lower-case(local-name())` XPath extension
/// function (a custom `calibre_xpath_extensions` function registered
/// solely so NCX parsing tolerates NCX documents using non-lowercase tag
/// names, which the DAISY NCX spec technically allows). [`Xml::opf_xpath`]
/// has no such extension-function support (out of scope, see its own
/// docs), so this is a small direct tree walk instead.
fn ci_children(xml: &Xml, id: XmlNodeId, local_lower: &str) -> Vec<XmlNodeId> {
    xml.element_children(id)
        .into_iter()
        .filter(|&c| {
            xml.local_name(c)
                .map(|l| l.eq_ignore_ascii_case(local_lower))
                .unwrap_or(false)
        })
        .collect()
}

/// Case-insensitive `descendant::*[local-name()=...]` lookup, in
/// document order. See [`ci_children`]'s docs.
fn ci_descendants(xml: &Xml, id: XmlNodeId, local_lower: &str, out: &mut Vec<XmlNodeId>) {
    for &c in xml.children(id) {
        if xml
            .local_name(c)
            .map(|l| l.eq_ignore_ascii_case(local_lower))
            .unwrap_or(false)
        {
            out.push(c);
        }
        ci_descendants(xml, c, local_lower, out);
    }
}

/// Port of `add_from_navpoint`.
fn add_from_navpoint(
    container: &Container,
    xml: &Xml,
    navpoint: XmlNodeId,
    toc: &mut Toc,
    parent: TocNodeId,
    ncx_name: &str,
) -> TocNodeId {
    let mut dest = None;
    let mut frag = None;
    let mut text: Option<String> = None;

    if let Some(&nl) = ci_children(xml, navpoint, "navlabel").first() {
        let mut t = String::new();
        for &txt in &ci_children(xml, nl, "text") {
            t.push_str(&xml.text_content(txt));
        }
        text = Some(t);
    }
    if let Some(&content) = ci_children(xml, navpoint, "content").first() {
        if let Some(href) = xml.get_attr(content, "src") {
            dest = container.href_to_name(href, Some(ncx_name));
            frag = url_fragment(href);
        }
    }
    let title = text.filter(|t| !t.is_empty());
    toc.add(parent, title, dest, frag)
}

/// Port of `process_ncx_node`.
fn process_ncx_node(
    container: &Container,
    xml: &Xml,
    node: XmlNodeId,
    toc: &mut Toc,
    toc_parent: TocNodeId,
    ncx_name: &str,
) {
    for navpoint in ci_children(xml, node, "navpoint") {
        let child = add_from_navpoint(container, xml, navpoint, toc, toc_parent, ncx_name);
        process_ncx_node(container, xml, navpoint, toc, child, ncx_name);
    }
}

/// Port of `parse_ncx`.
pub fn parse_ncx(container: &mut Container, ncx_name: &str) -> Result<Toc> {
    container.ensure_parsed(ncx_name)?;
    let mut toc = Toc::new();
    let xml = container.get_xml(ncx_name)?;

    let mut navmaps = Vec::new();
    ci_descendants(xml, xml.root, "navmap", &mut navmaps);
    if let Some(&navmap) = navmaps.first() {
        let root = toc.root;
        process_ncx_node(container, xml, navmap, &mut toc, root, ncx_name);
    }

    if let Some(root_elem) = xml.root_element() {
        for (k, v) in &xml.node(root_elem).attrs {
            if k.ends_with("lang") {
                toc.lang = Some(v.clone());
                break;
            }
        }
    }

    let mut metas = Vec::new();
    ci_descendants(xml, xml.root, "meta", &mut metas);
    for m in metas {
        if xml.get_attr(m, "name") == Some("dtb:uid") {
            if let Some(content) = xml.get_attr(m, "content") {
                if !content.is_empty() {
                    toc.uid = Some(content.to_string());
                    break;
                }
            }
        }
    }

    let mut pagelists = Vec::new();
    ci_descendants(xml, xml.root, "pagelist", &mut pagelists);
    for pl in pagelists {
        let mut pagetargets = Vec::new();
        ci_descendants(xml, pl, "pagetarget", &mut pagetargets);
        for pt in pagetargets {
            let Some(pagenum) = xml.get_attr(pt, "value") else {
                continue;
            };
            if pagenum.is_empty() {
                continue;
            }
            let mut contents = Vec::new();
            ci_descendants(xml, pt, "content", &mut contents);
            if let Some(&content) = contents.first() {
                if let Some(href) = xml.get_attr(content, "src") {
                    let dest = container.href_to_name(href, Some(ncx_name));
                    let frag = url_fragment(href);
                    toc.page_list.push(PageListEntry {
                        dest,
                        pagenum: pagenum.to_string(),
                        frag,
                    });
                }
            }
        }
    }

    Ok(toc)
}

// ===================================================================
// Nav-document parsing
// ===================================================================

fn first_child(dom: &Dom, parent: NodeId, tag: &str) -> Option<NodeId> {
    dom.children(parent)
        .into_iter()
        .find(|&c| dom.tag(c) == Some(tag))
}

/// `descendant-or-self::*/@title` joined with spaces, in document order
/// -- the fallback text source `add_from_li` uses when an `<a>`/`<span>`
/// has no text content of its own (icon-only links with a `title` attr).
fn collect_titles(dom: &Dom, id: NodeId) -> Vec<String> {
    dom.preorder_elements(id)
        .into_iter()
        .filter_map(|el| dom.node(el).attrs.get("title").cloned())
        .collect()
}

/// Port of `add_from_li`.
fn add_from_li(
    container: &Container,
    dom: &Dom,
    li: NodeId,
    toc: &mut Toc,
    parent: TocNodeId,
    nav_name: &str,
) -> TocNodeId {
    let mut dest = None;
    let mut frag = None;
    let mut text: Option<String> = None;

    for &child in &dom.children(li) {
        let tag = dom.tag(child);
        if tag != Some("a") && tag != Some("span") {
            continue;
        }
        let mut t = dom.text_content(child).trim().to_string();
        if t.is_empty() {
            t = collect_titles(dom, child).join(" ").trim().to_string();
        }
        if let Some(href) = dom.node(child).attrs.get("href").cloned() {
            dest = if href.starts_with('#') {
                Some(nav_name.to_string())
            } else {
                container.href_to_name(&href, Some(nav_name))
            };
            frag = url_fragment(&href);
        }
        text = Some(t);
        break;
    }
    let title = text.filter(|t| !t.is_empty());
    toc.add(parent, title, dest, frag)
}

/// Port of `process_nav_node`.
fn process_nav_node(
    container: &Container,
    dom: &Dom,
    node: NodeId,
    toc: &mut Toc,
    toc_parent: TocNodeId,
    nav_name: &str,
) {
    for li in dom
        .children(node)
        .into_iter()
        .filter(|&c| dom.tag(c) == Some("li"))
    {
        let child = add_from_li(container, dom, li, toc, toc_parent, nav_name);
        if let Some(ol) = first_child(dom, li, "ol") {
            process_nav_node(container, dom, ol, toc, child, nav_name);
        }
    }
}

/// Port of `parse_nav`.
pub fn parse_nav(container: &mut Container, nav_name: &str) -> Result<Toc> {
    container.ensure_parsed(nav_name)?;
    let mut toc = Toc::new();
    let dom = container.get_xhtml(nav_name)?;

    let mut seen_toc = false;
    let mut seen_pagelist = false;
    for nav in dom.find_all_tag_global("nav") {
        let Some(nt) = dom.node(nav).attrs.get("epub:type").cloned() else {
            continue;
        };
        if nt == "toc" && !seen_toc {
            if let Some(ol) = first_child(dom, nav, "ol") {
                seen_toc = true;
                let root = toc.root;
                process_nav_node(container, dom, ol, &mut toc, root, nav_name);
                for h in dom.children(nav) {
                    let Some(tag) = dom.tag(h) else { continue };
                    if !matches!(tag, "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
                        continue;
                    }
                    let mut text = dom.text_content(h);
                    if text.is_empty() {
                        text = dom.node(h).attrs.get("title").cloned().unwrap_or_default();
                    }
                    if !text.is_empty() {
                        toc.toc_title = Some(text);
                        break;
                    }
                }
            }
        } else if nt == "page-list" && !seen_pagelist {
            if let Some(ol) = first_child(dom, nav, "ol") {
                seen_pagelist = true;
                for li in dom
                    .children(ol)
                    .into_iter()
                    .filter(|&c| dom.tag(c) == Some("li"))
                {
                    for a in dom
                        .children(li)
                        .into_iter()
                        .filter(|&c| dom.tag(c) == Some("a"))
                    {
                        let Some(href) = dom.node(a).attrs.get("href").cloned() else {
                            continue;
                        };
                        if href.is_empty() {
                            continue;
                        }
                        let mut text = dom.text_content(a);
                        if text.is_empty() {
                            text = dom.node(a).attrs.get("title").cloned().unwrap_or_default();
                        }
                        let text = text.trim().to_string();
                        if text.is_empty() {
                            continue;
                        }
                        let (href_path, frag) = match href.split_once('#') {
                            Some((h, f)) => (h.to_string(), Some(f.to_string())),
                            None => (href.clone(), None),
                        };
                        let dest = if href.starts_with('#') {
                            Some(nav_name.to_string())
                        } else {
                            container.href_to_name(&href_path, Some(nav_name))
                        };
                        toc.page_list.push(PageListEntry {
                            dest,
                            pagenum: text,
                            frag: frag.filter(|f| !f.is_empty()),
                        });
                    }
                }
            }
        }
    }

    Ok(toc)
}

// ===================================================================
// Destination verification
// ===================================================================

fn collect_anchor_ids_dom(dom: &Dom) -> HashSet<String> {
    let mut set = HashSet::new();
    for el in dom.preorder_elements(dom.root) {
        if let Some(id) = dom.node(el).attrs.get("id") {
            set.insert(id.clone());
        }
        if dom.tag(el) == Some("a") {
            if let Some(name) = dom.node(el).attrs.get("name") {
                set.insert(name.clone());
            }
        }
    }
    set
}

fn collect_anchor_ids_xml(xml: &Xml) -> HashSet<String> {
    fn walk(xml: &Xml, id: XmlNodeId, set: &mut HashSet<String>) {
        if let Some(v) = xml.get_attr(id, "id") {
            set.insert(v.to_string());
        }
        if xml.local_name(id) == Some("a") {
            if let Some(v) = xml.get_attr(id, "name") {
                set.insert(v.to_string());
            }
        }
        for &c in xml.children(id) {
            walk(xml, c, set);
        }
    }
    let mut set = HashSet::new();
    walk(xml, xml.root, &mut set);
    set
}

/// The first element (in document order) with `id="{id_value}"`, walked
/// directly rather than via `Xml::opf_xpath` -- see the call site in
/// `commit_ncx_toc` for why a value-equality predicate can't go through
/// that engine. A private, narrower duplicate of `container.rs`'s own
/// private `find_by_id_attr` (same non-exported precedent as
/// `canonicalize_lang`'s independent copies).
fn find_by_id(xml: &Xml, id_value: &str) -> Option<XmlNodeId> {
    fn walk(xml: &Xml, node: XmlNodeId, id_value: &str) -> Option<XmlNodeId> {
        if xml.get_attr(node, "id") == Some(id_value) {
            return Some(node);
        }
        for &c in xml.children(node) {
            if let Some(found) = walk(xml, c, id_value) {
                return Some(found);
            }
        }
        None
    }
    walk(xml, xml.root, id_value)
}

/// Port of `verify_toc_destinations`.
pub fn verify_toc_destinations(container: &mut Container, toc: &mut Toc) -> Result<()> {
    let mut anchor_map: HashMap<String, HashSet<String>> = HashMap::new();
    for item in toc.iterdescendants(toc.root) {
        let dest = toc.node(item).dest.clone();
        let Some(name) = dest else {
            toc.node_mut(item).dest_exists = Some(false);
            toc.node_mut(item).dest_error = Some("No file named None exists".to_string());
            continue;
        };
        if !container.has_name(&name) {
            toc.node_mut(item).dest_exists = Some(false);
            toc.node_mut(item).dest_error = Some(format!("No file named {name} exists"));
            continue;
        }
        container.ensure_parsed(&name)?;
        let has_xpath = matches!(
            container.base.parsed_cache.get(&name),
            Some(ParsedItem::Xml(_)) | Some(ParsedItem::Xhtml(_))
        );
        if !has_xpath {
            toc.node_mut(item).dest_exists = Some(false);
            toc.node_mut(item).dest_error = Some(format!("No HTML file named {name} exists"));
            continue;
        }
        let frag = toc.node(item).frag.clone().filter(|f| !f.is_empty());
        let Some(frag) = frag else {
            toc.node_mut(item).dest_exists = Some(true);
            continue;
        };
        if !anchor_map.contains_key(&name) {
            let set = match container.base.parsed_cache.get(&name) {
                Some(ParsedItem::Xhtml(dom)) => collect_anchor_ids_dom(dom),
                Some(ParsedItem::Xml(xml)) => collect_anchor_ids_xml(xml),
                _ => HashSet::new(),
            };
            anchor_map.insert(name.clone(), set);
        }
        let exists = anchor_map[&name].contains(&frag);
        toc.node_mut(item).dest_exists = Some(exists);
        if !exists {
            toc.node_mut(item).dest_error =
                Some(format!("The anchor {frag} does not exist in file {name}"));
        }
    }
    Ok(())
}

// ===================================================================
// TOC discovery
// ===================================================================

/// Port of `find_existing_ncx_toc`. `//opf:spine/@toc` (an attribute-node
/// XPath) isn't expressible in [`Xml::opf_xpath`]'s narrow subset (see its
/// own docs), so this queries `//opf:spine[@toc]` (existence predicate)
/// and reads the attribute value directly instead -- equivalent result.
pub fn find_existing_ncx_toc(container: &mut Container) -> Result<Option<String>> {
    let spines = container.opf_xpath("//opf:spine[@toc]")?;
    let mut toc: Option<String> = None;
    if let Some(&spine) = spines.first() {
        let opf_name = container.opf_name.clone();
        let toc_id = container
            .get_xml(&opf_name)?
            .get_attr(spine, "toc")
            .map(|s| s.to_string());
        if let Some(toc_id) = toc_id {
            toc = container.manifest_id_map()?.get(&toc_id).cloned();
        }
    }
    if toc.is_none() {
        let ncx_mime = guess_type("a.ncx");
        toc = container
            .manifest_type_map()?
            .get(&ncx_mime)
            .and_then(|v| v.first())
            .cloned();
    }
    Ok(toc)
}

/// Port of `find_existing_nav_toc`.
pub fn find_existing_nav_toc(container: &mut Container) -> Result<Option<String>> {
    Ok(container
        .manifest_items_with_property("nav")?
        .into_iter()
        .next())
}

/// Port of `mark_as_nav`.
pub fn mark_as_nav(container: &mut Container, name: &str) -> Result<()> {
    if container.opf_version_parsed()?.0 > 2 {
        container.apply_unique_properties(Some(name), &["nav"])?;
    }
    Ok(())
}

/// Port of `get_x_toc`.
pub fn get_x_toc(
    container: &mut Container,
    find_toc: impl Fn(&mut Container) -> Result<Option<String>>,
    parse_toc: impl Fn(&mut Container, &str) -> Result<Toc>,
    verify_destinations: bool,
) -> Result<Toc> {
    let toc_name = find_toc(container)?;
    let usable = toc_name.as_deref().is_some_and(|n| container.has_name(n));
    let mut ans = if usable {
        parse_toc(container, toc_name.as_deref().unwrap())?
    } else {
        Toc::new()
    };
    ans.toc_file_name = if usable { toc_name } else { None };
    if verify_destinations {
        verify_toc_destinations(container, &mut ans)?;
    }
    Ok(ans)
}

/// Port of `get_toc`.
pub fn get_toc(container: &mut Container, verify_destinations: bool) -> Result<Toc> {
    let (major, _) = container.opf_version_parsed()?;
    if major < 3 {
        get_x_toc(
            container,
            find_existing_ncx_toc,
            parse_ncx,
            verify_destinations,
        )
    } else {
        let mut ans = get_x_toc(
            container,
            find_existing_nav_toc,
            parse_nav,
            verify_destinations,
        )?;
        if ans.is_empty(ans.root) {
            ans = get_x_toc(
                container,
                find_existing_ncx_toc,
                parse_ncx,
                verify_destinations,
            )?;
        }
        Ok(ans)
    }
}

// ===================================================================
// Landmarks
// ===================================================================

/// Port of `get_guide_landmarks`. Ported from `./opf:guide/opf:reference`
/// as `//opf:guide/opf:reference` -- [`Xml::opf_xpath`]'s subset has no
/// `.`-relative-to-context-node form (it always searches from the
/// document root), which is semantically equivalent here since
/// `<reference>` only ever legitimately appears under `<guide>` under
/// `<package>`.
pub fn get_guide_landmarks(container: &mut Container) -> Result<Vec<Landmark>> {
    let refs = container.opf_xpath("//opf:guide/opf:reference")?;
    let opf_name = container.opf_name.clone();
    let mut out = Vec::new();
    for r in refs {
        let (href, title, rtype) = {
            let xml = container.get_xml(&opf_name)?;
            (
                xml.get_attr(r, "href").unwrap_or("").to_string(),
                xml.get_attr(r, "title").map(|s| s.to_string()),
                xml.get_attr(r, "type").map(|s| s.to_string()),
            )
        };
        let (href, frag) = match href.split_once('#') {
            Some((h, f)) => (h.to_string(), f.to_string()),
            None => (href, String::new()),
        };
        if let Some(name) = container.href_to_name(&href, Some(&opf_name)) {
            if container.has_name(&name) {
                out.push(Landmark {
                    dest: name,
                    frag,
                    title: title.unwrap_or_default(),
                    r#type: rtype.unwrap_or_default(),
                });
            }
        }
    }
    Ok(out)
}

/// Port of `get_nav_landmarks`.
pub fn get_nav_landmarks(container: &mut Container) -> Result<Vec<Landmark>> {
    let mut out = Vec::new();
    let Some(nav_name) = find_existing_nav_toc(container)? else {
        return Ok(out);
    };
    if !container.has_name(&nav_name) {
        return Ok(out);
    }
    container.ensure_parsed(&nav_name)?;
    let dom = container.get_xhtml(&nav_name)?;
    for elem in dom.find_all_tag_global("nav") {
        if dom.node(elem).attrs.get("epub:type").map(|s| s.as_str()) != Some("landmarks") {
            continue;
        }
        for li in dom.find_all_tag(elem, "li") {
            let Some(&a) = dom.find_all_tag(li, "a").first() else {
                continue;
            };
            let Some(href) = dom.node(a).attrs.get("href").cloned() else {
                continue;
            };
            let rtype = dom.node(a).attrs.get("epub:type").cloned();
            let title = dom.text_content(a).trim().to_string();
            let (href, frag) = match href.split_once('#') {
                Some((h, f)) => (h.to_string(), f.to_string()),
                None => (href, String::new()),
            };
            if let Some(name) = container.href_to_name(&href, Some(&nav_name)) {
                if container.has_name(&name) {
                    out.push(Landmark {
                        dest: name,
                        frag,
                        title,
                        r#type: rtype.unwrap_or_default(),
                    });
                }
            }
        }
    }
    Ok(out)
}

/// Port of `get_landmarks`.
pub fn get_landmarks(container: &mut Container) -> Result<Vec<Landmark>> {
    let (major, _) = container.opf_version_parsed()?;
    if major < 3 {
        return get_guide_landmarks(container);
    }
    let ans = get_nav_landmarks(container)?;
    if ans.is_empty() {
        return get_guide_landmarks(container);
    }
    Ok(ans)
}

// ===================================================================
// TOC generation from content (headings / links / files)
// ===================================================================

/// Port of `ensure_id`.
fn ensure_id(dom: &mut Dom, elem: NodeId, all_ids: &mut HashSet<String>) -> (bool, String) {
    if let Some(id) = dom.node(elem).attrs.get("id") {
        if !id.is_empty() {
            return (false, id.clone());
        }
    }
    if dom.tag(elem) == Some("a") {
        if let Some(anchor) = dom.node(elem).attrs.get("name").cloned() {
            if !anchor.is_empty() {
                dom.node_mut(elem)
                    .attrs
                    .insert("id".to_string(), anchor.clone());
                return (false, anchor);
            }
        }
    }
    let mut c = 0u32;
    loop {
        c += 1;
        let q = format!("toc_{c}");
        if !all_ids.contains(&q) {
            dom.node_mut(elem).attrs.insert("id".to_string(), q.clone());
            all_ids.insert(q.clone());
            return (true, q);
        }
    }
}

/// Port of `elem_to_toc_text`. `pub(crate)` since `docx::toc` (issue
/// #292) imports it too, matching Python's own `from
/// calibre.ebooks.oeb.polish.toc import elem_to_toc_text`.
pub(crate) fn elem_to_toc_text(dom: &Dom, elem: NodeId, prefer_title: bool) -> String {
    let mut text = dom.text_content(elem).trim().to_string();
    if prefer_title {
        let title_trim = dom
            .node(elem)
            .attrs
            .get("title")
            .map(|t| t.trim().to_string())
            .unwrap_or_default();
        if !title_trim.is_empty() {
            text = title_trim;
        }
    }
    if text.is_empty() {
        text = dom
            .node(elem)
            .attrs
            .get("title")
            .cloned()
            .unwrap_or_default();
    }
    if text.is_empty() {
        text = dom.node(elem).attrs.get("alt").cloned().unwrap_or_default();
    }
    text = whitespace_re().replace_all(text.trim(), " ").into_owned();
    text = text
        .chars()
        .take(1000)
        .collect::<String>()
        .trim()
        .to_string();
    if text.is_empty() {
        text = "(Untitled)".to_string();
    }
    text
}

/// Port of `item_at_top`. Because this arena represents lxml's
/// `.text`/`.tail` as ordinary sibling `Text` nodes (see `crate::dom`'s
/// module docs), a plain document-order pre-order walk that stops at
/// `elem` naturally reproduces Python's "ancestor-path"-guarded tail
/// check: a `Text` node that would be an *ancestor's* tail in lxml is,
/// here, a sibling that appears *after* the ancestor's entire subtree
/// (including `elem`) closes -- so it is never visited before `elem` is
/// reached.
fn item_at_top(dom: &Dom, elem: NodeId) -> bool {
    let Some(body) = dom.find_first_tag_global("body") else {
        return false;
    };
    enum Stop {
        Reached,
        Content,
    }
    fn walk(dom: &Dom, id: NodeId, target: NodeId) -> std::result::Result<(), Stop> {
        if id == target {
            return Err(Stop::Reached);
        }
        match &dom.node(id).kind {
            NodeKind::Text(t) => {
                if !t.trim().is_empty() {
                    return Err(Stop::Content);
                }
            }
            NodeKind::Element(tag) => {
                if tag == "img" {
                    return Err(Stop::Content);
                }
                for &c in &dom.node(id).children {
                    walk(dom, c, target)?;
                }
            }
            NodeKind::Comment(_) | NodeKind::Document => {}
        }
        Ok(())
    }
    !matches!(walk(dom, body, elem), Err(Stop::Content))
}

/// A narrow XPath subset for [`Dom`] content documents, covering exactly
/// the shape real `from_xpaths` callers use: `//prefix:tag` or `//tag`
/// (optionally with `[@attr]`/`[@attr1 and @attr2]` existence-only
/// predicates). A namespace prefix (e.g. calibre's own `//h:h1`) is
/// accepted and ignored -- `Dom` is HTML5-tag-soup-parsed and does not
/// track XML namespace prefixes (see `crate::dom`'s module docs), so a
/// prefix on the tag name can only ever mean "the XHTML namespace",
/// which every element in an XHTML content document already is. This is
/// not a general XPath engine -- the same documented scope boundary as
/// [`Xml::opf_xpath`]; calibre's GUI additionally lets a user type
/// *arbitrary* XPath into a "Table of Contents from XPath" wizard
/// (functions/axes/value-equality predicates this subset doesn't parse),
/// which is out of scope for this port, the same class of gap as the
/// CSS-parser-shaped gaps documented in `pretty.rs`/`utils.rs`.
fn dom_xpath_all(dom: &Dom, expr: &str) -> Vec<NodeId> {
    let body = expr.strip_prefix("//").unwrap_or(expr);
    let (tag_part, predicate) = match body.find('[') {
        None => (body, None),
        Some(open) => {
            let tag = &body[..open];
            let inner = body[open + 1..].trim_end_matches(']');
            let attrs: Vec<&str> = inner
                .split(" and ")
                .filter_map(|p| p.trim().strip_prefix('@'))
                .collect();
            (tag, Some(attrs))
        }
    };
    let tag = tag_part
        .rsplit_once(':')
        .map(|(_, t)| t)
        .unwrap_or(tag_part);
    let mut out: Vec<NodeId> = if tag == "*" {
        dom.preorder_elements(dom.root)
    } else {
        dom.find_all_tag_global(tag)
    };
    if let Some(attrs) = predicate {
        out.retain(|&id| attrs.iter().all(|a| dom.node(id).attrs.contains_key(*a)));
    }
    out
}

fn parent_for_level(
    toc: &Toc,
    node_level_map: &HashMap<TocNodeId, usize>,
    child_level: usize,
) -> TocNodeId {
    let limit = child_level as i64 - 1;
    fn process(
        toc: &Toc,
        node_level_map: &HashMap<TocNodeId, usize>,
        node: TocNodeId,
        limit: i64,
    ) -> TocNodeId {
        let Some(child) = toc.last_child(node) else {
            return node;
        };
        let lvl = *node_level_map.get(&child).unwrap_or(&0) as i64;
        if lvl > limit {
            node
        } else if lvl == limit {
            child
        } else {
            process(toc, node_level_map, child, limit)
        }
    }
    process(toc, node_level_map, toc.root, limit)
}

/// Port of `from_xpaths`. `xpaths` uses the narrow subset documented on
/// [`dom_xpath_all`], not full XPath.
pub fn from_xpaths(container: &mut Container, xpaths: &[&str], prefer_title: bool) -> Result<Toc> {
    let mut toc = Toc::new();
    let spine: Vec<String> = container
        .spine_names()?
        .into_iter()
        .map(|(n, _)| n)
        .collect();

    let mut maps: Vec<(String, HashMap<usize, Vec<NodeId>>)> = Vec::new();
    let mut empty_levels: HashSet<usize> = (1..=xpaths.len()).collect();
    for name in &spine {
        container.ensure_parsed(name)?;
        let dom = container.get_xhtml(name)?;
        let mut level_map: HashMap<usize, Vec<NodeId>> = HashMap::new();
        for (i, xp) in xpaths.iter().enumerate() {
            let lvl = i + 1;
            let matches = dom_xpath_all(dom, xp);
            if !matches.is_empty() {
                empty_levels.remove(&lvl);
            }
            level_map.insert(lvl, matches);
        }
        maps.push((name.clone(), level_map));
    }

    if !empty_levels.is_empty() {
        for (_, lmap) in maps.iter_mut() {
            let mut kept: Vec<(usize, Vec<NodeId>)> = lmap
                .iter()
                .filter(|(lvl, _)| !empty_levels.contains(lvl))
                .map(|(lvl, v)| (*lvl, v.clone()))
                .collect();
            kept.sort_by_key(|(lvl, _)| *lvl);
            *lmap = kept
                .into_iter()
                .enumerate()
                .map(|(i, (_, v))| (i + 1, v))
                .collect();
        }
    }

    let mut node_level_map: HashMap<TocNodeId, usize> = HashMap::new();
    node_level_map.insert(toc.root, 0);

    for (name, level_item_map) in &maps {
        let mut item_level_map: HashMap<NodeId, usize> = HashMap::new();
        for (&lvl, elems) in level_item_map {
            for &e in elems {
                item_level_map.insert(e, lvl);
            }
        }

        let dom = container.get_xhtml_mut(name)?;
        let mut all_ids: HashSet<String> = HashSet::new();
        for el in dom.preorder_elements(dom.root) {
            if let Some(id) = dom.node(el).attrs.get("id") {
                all_ids.insert(id.clone());
            }
        }
        let order = dom.preorder_elements(dom.root);

        let mut item_dirtied = false;
        let mut pending: Vec<(usize, String, Option<String>)> = Vec::new();
        for item in order {
            let Some(&lvl) = item_level_map.get(&item) else {
                continue;
            };
            let text = elem_to_toc_text(dom, item, prefer_title);
            let elem_id = if item_at_top(dom, item) {
                None
            } else {
                let (dirtied, id) = ensure_id(dom, item, &mut all_ids);
                item_dirtied |= dirtied;
                Some(id)
            };
            pending.push((lvl, text, elem_id));
        }

        if item_dirtied {
            container.commit_item(name, true)?;
        }

        for (lvl, text, elem_id) in pending {
            let parent = parent_for_level(&toc, &node_level_map, lvl);
            let child = toc.add(parent, Some(text), Some(name.clone()), elem_id);
            node_level_map.insert(child, lvl);
            toc.node_mut(child).dest_exists = Some(true);
        }
    }

    Ok(toc)
}

/// Port of `from_links`.
pub fn from_links(container: &mut Container) -> Result<Toc> {
    let mut toc = Toc::new();
    let mut seen_titles: HashSet<String> = HashSet::new();
    let mut seen_dests: HashSet<(Option<String>, Option<String>)> = HashSet::new();
    let spine = container.spine_names()?;
    for (name, _is_linear) in &spine {
        container.ensure_parsed(name)?;
        let dom = container.get_xhtml(name)?;
        for a in dom.find_all_tag_global("a") {
            let Some(href) = dom.node(a).attrs.get("href").cloned() else {
                continue;
            };
            if href.trim().is_empty() {
                continue;
            }
            let (dest, frag): (Option<String>, Option<String>) =
                if let Some(stripped) = href.strip_prefix('#') {
                    (Some(name.clone()), Some(stripped.to_string()))
                } else {
                    let (h, f) = match href.split_once('#') {
                        Some((h, f)) => (h.to_string(), Some(f.to_string())),
                        None => (href.clone(), None),
                    };
                    (container.href_to_name(&h, Some(name)), f)
                };
            let frag = frag.filter(|f| !f.is_empty());
            let key = (dest.clone(), frag.clone());
            if seen_dests.contains(&key) {
                continue;
            }
            seen_dests.insert(key);
            let text = elem_to_toc_text(dom, a, false);
            if seen_titles.contains(&text) {
                continue;
            }
            seen_titles.insert(text.clone());
            toc.add(toc.root, Some(text), dest, frag);
        }
    }
    verify_toc_destinations(container, &mut toc)?;
    let to_remove: Vec<TocNodeId> = toc
        .children(toc.root)
        .iter()
        .copied()
        .filter(|&c| toc.node(c).dest_exists != Some(true))
        .collect();
    for c in to_remove {
        toc.remove(toc.root, c);
    }
    Ok(toc)
}

/// Port of `find_text`.
fn find_text(dom: &Dom, node: NodeId) -> Option<String> {
    const LIMIT: usize = 200;
    for &child in &dom.node(node).children {
        if !matches!(dom.node(child).kind, NodeKind::Element(_)) {
            continue;
        }
        let text = dom.text_content(child).trim().to_string();
        let text = whitespace_re().replace_all(&text, " ").into_owned();
        if text.chars().count() < 1 {
            continue;
        }
        if text.chars().count() > LIMIT {
            let ntext = find_text(dom, child);
            return Some(ntext.unwrap_or_else(|| {
                let truncated: String = text.chars().take(LIMIT).collect();
                format!("{truncated}...")
            }));
        }
        return Some(text);
    }
    None
}

/// Port of `from_files`.
pub fn from_files(container: &mut Container) -> Result<Toc> {
    let mut toc = Toc::new();
    let spine: Vec<String> = container
        .spine_names()?
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    for (i, name) in spine.iter().enumerate() {
        container.ensure_parsed(name)?;
        let dom = container.get_xhtml(name)?;
        let Some(body) = dom.find_first_tag_global("body") else {
            continue;
        };
        let mut text = find_text(dom, body);
        if text.as_deref().map(|t| t.is_empty()).unwrap_or(true) {
            let base = name.rsplit('/').next().unwrap_or(name.as_str());
            let stem = match base.rsplit_once('.') {
                Some((s, _)) => s,
                None => "",
            };
            text = Some(
                if i == 0 && matches!(stem.to_lowercase().as_str(), "titlepage" | "cover") {
                    "Cover".to_string()
                } else {
                    base.to_string()
                },
            );
        }
        toc.add(toc.root, text, Some(name.clone()), None);
    }
    Ok(toc)
}

// ===================================================================
// Adding a TOC entry at a clicked location (GUI click-to-add-entry)
// ===================================================================

/// `pub(crate)`: also used directly by `oeb::polish::split`'s [`split`]
/// (the ported `split.py`'s `from calibre.ebooks.oeb.polish.toc import
/// node_from_loc`) to resolve a click-to-split location the same way a
/// click-to-add-TOC-entry location is resolved here.
///
/// [`split`]: super::split::split
pub(crate) fn node_from_loc(dom: &Dom, locs: &[usize], totals: Option<&[usize]>) -> Result<NodeId> {
    let mut node = dom
        .find_first_tag_global("body")
        .ok_or_else(|| PolishError::MalformedMarkup("document has no <body>".to_string()))?;
    for (i, &loc) in locs.iter().enumerate() {
        let children: Vec<NodeId> = dom
            .node(node)
            .children
            .iter()
            .copied()
            .filter(|&c| matches!(dom.node(c).kind, NodeKind::Element(_)))
            .collect();
        if let Some(t) = totals {
            if t.get(i) != Some(&children.len()) {
                return Err(
                    PolishError::MalformedMarkup("child count mismatch".to_string()).into(),
                );
            }
        }
        node = *children.get(loc).ok_or_else(|| {
            PolishError::MalformedMarkup("location index out of range".to_string())
        })?;
    }
    Ok(node)
}

/// Port of `add_id`. Python retries with `force_html5_parse=True` on a
/// [`PolishError::MalformedMarkup`] from the first attempt, because its
/// first attempt uses a *strict* XML parse that can genuinely produce a
/// different tree shape than the HTML5 tag-soup parse (e.g. documents
/// with nested `<p>` tags). This port's `parsing.rs` always takes the
/// tag-soup path unconditionally (a documented design decision -- see
/// its module docs), so there is no second, differently-shaped parse to
/// retry with here: a location mismatch means the caller's cached
/// child-count snapshot is stale relative to the current document, which
/// no reparse strategy available to this port can recover from.
pub fn add_id(
    container: &mut Container,
    name: &str,
    locs: &[usize],
    totals: Option<&[usize]>,
) -> Result<String> {
    container.ensure_parsed(name)?;
    let node = node_from_loc(container.get_xhtml(name)?, locs, totals).map_err(|_| {
        PolishError::MalformedMarkup(format!(
            "The file {name} has malformed markup. Try running the Fix HTML tool before editing."
        ))
    })?;
    let has_id = container
        .get_xhtml(name)?
        .node(node)
        .attrs
        .get("id")
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    if !has_id {
        let dom = container.get_xhtml_mut(name)?;
        let mut all_ids: HashSet<String> = HashSet::new();
        for el in dom.preorder_elements(dom.root) {
            if let Some(id) = dom.node(el).attrs.get("id") {
                all_ids.insert(id.clone());
            }
        }
        ensure_id(dom, node, &mut all_ids);
    }
    container.commit_item(name, true)?;
    Ok(container
        .get_xhtml(name)?
        .node(node)
        .attrs
        .get("id")
        .cloned()
        .unwrap_or_default())
}

// ===================================================================
// NCX generation / commit
// ===================================================================

const NCX_NS: &str = crate::oeb::constants::NCX_NS;

/// Port of `create_ncx`. `to_href` mirrors Python's
/// `partial(container.name_to_href, base=tocname)`.
///
/// **Known limitation inherited from `xmltree.rs`:** that arena stores
/// attributes by local name only (see its module docs) and does not
/// track attribute namespaces, so the `xml:lang` attribute this function
/// sets on `<ncx>` round-trips correctly through this crate's own
/// parse/serialize (both read and write agree it's keyed `"lang"`) but
/// serializes as a bare `lang="..."` rather than the namespaced
/// `xml:lang="..."` the DAISY NCX spec (and epubcheck) requires.
pub fn create_ncx(
    toc: &Toc,
    mut to_href: impl FnMut(&str) -> String,
    btitle: &str,
    lang: &str,
    uid: &str,
) -> Result<Xml> {
    let lang = lang.replace('_', "-");
    let mut ncx = Xml::parse(&format!("<ncx xmlns=\"{NCX_NS}\" version=\"2005-1\"/>"))?;
    let root = ncx
        .root_element()
        .ok_or_else(|| anyhow::anyhow!("freshly parsed NCX skeleton has no root element"))?;
    ncx.set_attr(root, "lang", lang);

    let head = ncx.new_element("head", Some(NCX_NS));
    ncx.insert_element(root, head, None);

    let add_meta = |ncx: &mut Xml, name: &str, content: String| {
        let meta = ncx.new_element("meta", Some(NCX_NS));
        ncx.set_attr(meta, "name", name);
        ncx.set_attr(meta, "content", content);
        ncx.insert_element(head, meta, None);
    };
    add_meta(&mut ncx, "dtb:uid", uid.to_string());
    add_meta(&mut ncx, "dtb:depth", toc.depth(toc.root).to_string());
    add_meta(
        &mut ncx,
        "dtb:generator",
        format!("calibre ({})", calibre_utils::constants::VERSION),
    );
    add_meta(&mut ncx, "dtb:totalPageCount", "0".to_string());
    add_meta(&mut ncx, "dtb:maxPageNumber", "0".to_string());

    let doc_title = ncx.new_element("docTitle", Some(NCX_NS));
    ncx.insert_element(root, doc_title, None);
    let title_text = ncx.new_element("text", Some(NCX_NS));
    ncx.insert_element(doc_title, title_text, None);
    ncx.set_element_text(title_text, btitle);

    let navmap = ncx.new_element("navMap", Some(NCX_NS));
    ncx.insert_element(root, navmap, None);

    fn process_node(
        ncx: &mut Xml,
        toc: &Toc,
        xml_parent: XmlNodeId,
        toc_parent: TocNodeId,
        play_order: &mut u32,
        to_href: &mut impl FnMut(&str) -> String,
    ) {
        for &child in toc.children(toc_parent) {
            *play_order += 1;
            let point = ncx.new_element("navPoint", Some(NCX_NS));
            ncx.set_attr(point, "id", format!("num_{play_order}"));
            ncx.set_attr(point, "playOrder", play_order.to_string());
            ncx.insert_element(xml_parent, point, None);

            let label = ncx.new_element("navLabel", Some(NCX_NS));
            ncx.insert_element(point, label, None);
            let text_el = ncx.new_element("text", Some(NCX_NS));
            ncx.insert_element(label, text_el, None);
            if let Some(title) = &toc.node(child).title {
                let title = whitespace_re().replace_all(title, " ").into_owned();
                ncx.set_element_text(text_el, title);
            }

            if let Some(dest) = toc.node(child).dest.clone() {
                let mut href = to_href(&dest);
                if let Some(frag) = &toc.node(child).frag {
                    href.push('#');
                    href.push_str(frag);
                }
                let content = ncx.new_element("content", Some(NCX_NS));
                ncx.set_attr(content, "src", href);
                ncx.insert_element(point, content, None);
            }
            process_node(ncx, toc, point, child, play_order, to_href);
        }
    }
    let mut play_order = 0u32;
    process_node(
        &mut ncx,
        toc,
        navmap,
        toc.root,
        &mut play_order,
        &mut to_href,
    );

    Ok(ncx)
}

/// Port of `commit_ncx_toc`.
pub fn commit_ncx_toc(
    container: &mut Container,
    toc: &Toc,
    lang: Option<&str>,
    uid: Option<&str>,
) -> Result<()> {
    let mut tocname = find_existing_ncx_toc(container)?;
    if tocname.is_none() {
        let item = container.generate_item("toc.ncx", "toc", None, true)?;
        let opf_name = container.opf_name.clone();
        let (href, ncx_id) = {
            let xml = container.get_xml(&opf_name)?;
            (
                xml.get_attr(item, "href").unwrap_or("").to_string(),
                xml.get_attr(item, "id").unwrap_or("").to_string(),
            )
        };
        tocname = container.href_to_name(&href, Some(&opf_name));
        let spines = container.opf_xpath("//opf:spine")?;
        {
            let xml = container.get_xml_mut(&opf_name)?;
            for s in spines {
                xml.set_attr(s, "toc", ncx_id.clone());
            }
        }
        container.dirty(&opf_name);
    }
    let tocname =
        tocname.ok_or_else(|| anyhow::anyhow!("Failed to create or locate an NCX TOC file"))?;

    let mut lang = lang.map(|s| s.to_string()).filter(|s| !s.is_empty());
    if lang.is_none() {
        let mut l = get_lang();
        for lnode in container.opf_xpath("//dc:language")? {
            let opf_name = container.opf_name.clone();
            let text = container
                .get_xml(&opf_name)?
                .element_text(lnode)
                .unwrap_or("")
                .trim()
                .to_string();
            if let Some(canon) = canonicalize_lang(&text) {
                l = lang_as_iso639_1(&canon).unwrap_or(canon);
                break;
            }
        }
        lang = Some(l);
    }
    let mut lang = lang.unwrap_or_default();
    lang = lang_as_iso639_1(&lang).unwrap_or(lang);

    let mut uid = uid.map(|s| s.to_string()).filter(|s| !s.is_empty());
    if uid.is_none() {
        let mut u = uuid_id();
        let opf_name = container.opf_name.clone();
        let eid = {
            let root = container.opf_root()?;
            container
                .get_xml(&opf_name)?
                .get_attr(root, "unique-identifier")
                .map(|s| s.to_string())
        };
        if let Some(eid) = eid {
            // `//*[@id="{eid}"]` (a value-equality predicate) is not
            // expressible via `Container::opf_xpath` -- its underlying
            // `Xml::opf_xpath` subset only supports attribute-existence
            // predicates (`[@attr]`), by its own documented design (see
            // `xmltree.rs`'s docs). Walk the tree directly instead.
            let m = { find_by_id(container.get_xml(&opf_name)?, &eid) };
            if let Some(m) = m {
                u = container
                    .get_xml(&opf_name)?
                    .element_text(m)
                    .unwrap_or("")
                    .to_string();
            }
        }
        uid = Some(u);
    }
    let uid = uid.unwrap_or_default();

    let mut title = "Table of Contents".to_string();
    if let Some(&m) = container.opf_xpath("//dc:title")?.first() {
        let opf_name = container.opf_name.clone();
        let x = container
            .get_xml(&opf_name)?
            .element_text(m)
            .unwrap_or("")
            .trim()
            .to_string();
        if !x.is_empty() {
            title = x;
        }
    }

    let ncx = {
        let tocname_for_href = tocname.clone();
        create_ncx(
            toc,
            |dest| container.name_to_href(dest, Some(&tocname_for_href)),
            &title,
            &lang,
            &uid,
        )?
    };
    container
        .base
        .parsed_cache
        .insert(tocname.clone(), ParsedItem::Xml(ncx));
    container.dirty(&tocname);
    container.pretty_print.insert(tocname);
    Ok(())
}

// ===================================================================
// Nav-document generation / commit
// ===================================================================

const NEW_NAV_TEMPLATE: &str = "<?xml version='1.0' encoding='utf-8'?>\n\
<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\">\n\
    <head>\n\
        <title>Navigation</title>\n\
    </head>\n\
\n\
    <body>\n\
    </body>\n\
</html>";

const INLINE_TOC_STYLES_CSS: &str =
    "li { list-style-type: none; padding-left: 2em; margin-left: 0}\n\na { text-decoration: none }\n\na:hover { color: red }\n";

fn set_dom_tail(dom: &mut Dom, id: NodeId, text: &str) {
    let Some(parent) = dom.parent(id) else {
        return;
    };
    let Some(pos) = dom.index_in_parent(id) else {
        return;
    };
    let next = dom.children(parent).get(pos + 1).copied();
    if let Some(next) = next {
        if let NodeKind::Text(_) = dom.node(next).kind {
            dom.node_mut(next).kind = NodeKind::Text(text.to_string());
            return;
        }
    }
    let t = dom.new_text(text);
    dom.insert_child(parent, pos + 1, t);
}

fn set_dom_leading_text(dom: &mut Dom, id: NodeId, text: &str) {
    if let Some(&first) = dom.children(id).first() {
        if let NodeKind::Text(_) = dom.node(first).kind {
            dom.node_mut(first).kind = NodeKind::Text(text.to_string());
            return;
        }
    }
    let t = dom.new_text(text);
    dom.insert_child(id, 0, t);
}

/// Port of `ensure_single_nav_of_type`.
fn ensure_single_nav_of_type(dom: &mut Dom, ntype: &str) -> NodeId {
    let mut navs: Vec<NodeId> = dom
        .find_all_tag_global("nav")
        .into_iter()
        .filter(|&n| dom.node(n).attrs.get("epub:type").map(|s| s.as_str()) == Some(ntype))
        .collect();
    if navs.len() > 1 {
        for &x in &navs[1..] {
            dom.detach(x);
        }
        navs.truncate(1);
    }
    let nav = if let Some(&nav) = navs.first() {
        let children: Vec<NodeId> = dom.node(nav).children.clone();
        for c in children {
            dom.detach(c);
        }
        nav
    } else {
        let nav = dom.new_element("nav");
        if let Some(body) = dom.find_first_tag_global("body") {
            dom.append_child(body, nav);
        }
        nav
    };
    dom.node_mut(nav)
        .attrs
        .insert("epub:type".to_string(), ntype.to_string());
    nav
}

/// Port of `collapse_li`.
fn collapse_li_dom(dom: &mut Dom, parent: NodeId) {
    for li in dom.find_all_tag(parent, "li") {
        let elem_children: Vec<NodeId> = dom
            .node(li)
            .children
            .iter()
            .copied()
            .filter(|&c| matches!(dom.node(c).kind, NodeKind::Element(_)))
            .collect();
        if elem_children.len() != 1 {
            continue;
        }
        if let Some(&first) = dom.node(li).children.first() {
            if matches!(dom.node(first).kind, NodeKind::Text(_)) {
                dom.detach(first);
            }
        }
        let only = elem_children[0];
        if let Some(next) = dom.next_sibling(only) {
            if matches!(dom.node(next).kind, NodeKind::Text(_)) {
                dom.detach(next);
            }
        }
    }
}

/// Port of `create_nav_li`, taking `root_path` instead of `&Container`
/// so it can be called while a `&mut Dom` borrowed from the container is
/// already held (see call sites).
fn create_nav_li_dom(
    dom: &mut Dom,
    ol: NodeId,
    dest: &str,
    frag: Option<&str>,
    root_path: &Path,
    tocname: &str,
) -> NodeId {
    let li = dom.new_element("li");
    dom.append_child(ol, li);
    let a = dom.new_element("a");
    dom.append_child(li, a);
    let mut href = name_to_href_at(dest, root_path, Some(tocname));
    if let Some(f) = frag {
        if !f.is_empty() {
            href.push('#');
            href.push_str(f);
        }
    }
    dom.node_mut(a).attrs.insert("href".to_string(), href);
    a
}

/// Port of `ensure_container_has_nav`. Returns just the resolved
/// `tocname` (rather than Python's `(tocname, root)` pair) -- the caller
/// re-fetches the `Dom` from the container as needed, since Rust can't
/// alias a `&mut Dom` living inside `container`'s parse cache the way a
/// Python object reference can. `previous_nav` (an already-parsed `Dom`
/// a caller wants reused as the freshly-generated nav's starting point,
/// e.g. `upgrade.py`'s EPUB2-to-3 conversion carrying over a
/// user-authored nav template) is accepted for signature parity with
/// real callers outside this file's own scope; ported for real, exercised
/// by this file's own tests via the `None` path.
pub fn ensure_container_has_nav(
    container: &mut Container,
    lang: Option<&str>,
    previous_nav: Option<(String, Dom)>,
) -> Result<String> {
    let mut tocname = find_existing_nav_toc(container)?;
    let mut previous_nav_href = None;
    let mut previous_nav_root = None;
    if let Some((href, dom)) = previous_nav {
        if let Some(nav_name) = container.href_to_name(&href, None) {
            if container.exists(&nav_name) {
                tocname = Some(nav_name.clone());
                container.apply_unique_properties(Some(&nav_name), &["nav"])?;
            }
        }
        previous_nav_href = Some(href);
        previous_nav_root = Some(dom);
    }

    let tocname = match tocname {
        Some(name) => name,
        None => {
            let name = previous_nav_href.unwrap_or_else(|| "nav.xhtml".to_string());
            let item = container.generate_item(&name, "nav", None, true)?;
            let opf_name = container.opf_name.clone();
            {
                let xml = container.get_xml_mut(&opf_name)?;
                xml.set_attr(item, "properties", "nav");
            }
            container.dirty(&opf_name);
            let href = container
                .get_xml(&opf_name)?
                .get_attr(item, "href")
                .unwrap_or("")
                .to_string();
            let new_name = container
                .href_to_name(&href, Some(&opf_name))
                .ok_or_else(|| anyhow::anyhow!("Failed to resolve generated nav document href"))?;
            let root = previous_nav_root.unwrap_or_else(|| Dom::parse(NEW_NAV_TEMPLATE));
            container
                .base
                .parsed_cache
                .insert(new_name.clone(), ParsedItem::Xhtml(root));
            container.dirty(&new_name);
            new_name
        }
    };
    container.ensure_parsed(&tocname)?;

    if let Some(lang) = lang {
        let lang = lang_as_iso639_1(lang).unwrap_or_else(|| lang.to_string());
        let dom = container.get_xhtml_mut(&tocname)?;
        if let Some(html) = dom.find_first_tag_global("html") {
            dom.node_mut(html)
                .attrs
                .insert("lang".to_string(), lang.clone());
            dom.node_mut(html)
                .attrs
                .insert("xml:lang".to_string(), lang);
        }
        container.dirty(&tocname);
    }
    Ok(tocname)
}

/// Port of `commit_nav_toc`. `landmarks` is accepted (matching Python's
/// signature exactly) but -- as in the original -- never referenced in
/// the body; landmark generation is [`set_landmarks`], a distinct,
/// separately-called function (real callers, e.g.
/// `gui2/tweak_book/widgets.py`, call it directly, not through this
/// one).
pub fn commit_nav_toc(
    container: &mut Container,
    toc: &Toc,
    lang: Option<&str>,
    _landmarks: Option<&[Landmark]>,
    previous_nav: Option<(String, Dom)>,
) -> Result<()> {
    let tocname = ensure_container_has_nav(container, lang, previous_nav)?;
    let root_path = container.root.clone();

    let valid_page_list: Vec<(String, Option<String>, String)> = toc
        .page_list
        .iter()
        .filter(|e| {
            e.dest
                .as_deref()
                .map(|d| {
                    container.has_name(d)
                        && container
                            .base
                            .mime_map
                            .get(d)
                            .map(|m| OEB_DOCS.contains(&m.as_str()))
                            .unwrap_or(false)
                })
                .unwrap_or(false)
        })
        .map(|e| (e.dest.clone().unwrap(), e.frag.clone(), e.pagenum.clone()))
        .collect();

    container.ensure_parsed(&tocname)?;
    let dom = container.get_xhtml_mut(&tocname)?;

    let nav = ensure_single_nav_of_type(dom, "toc");
    if let Some(title) = &toc.toc_title {
        let h1 = dom.new_element("h1");
        let t = dom.new_text(title);
        dom.append_child(h1, t);
        dom.append_child(nav, h1);
    }

    let rnode = dom.new_element("ol");
    dom.append_child(nav, rnode);

    // Note: no manual leading/tail-text bookkeeping is needed here (unlike
    // `toc_to_html`'s `process_node`) -- the `pretty_dom_xml_tree` call
    // below is the *unconditional*, every-level recursive indenter (port
    // of `pretty_xml_tree`, not the block-tag-gated `pretty_html_tree`
    // `toc_to_html` uses), so it re-derives every node's whitespace from
    // scratch regardless of what's set here.
    fn process_node(
        dom: &mut Dom,
        toc: &Toc,
        xml_parent: NodeId,
        toc_parent: TocNodeId,
        root_path: &Path,
        tocname: &str,
    ) {
        for &child in toc.children(toc_parent) {
            let li = dom.new_element("li");
            dom.append_child(xml_parent, li);
            let title = toc.node(child).title.clone().unwrap_or_default();
            let title = whitespace_re().replace_all(&title, " ").trim().to_string();
            let has_dest = toc.node(child).dest.is_some();
            let a = dom.new_element(if has_dest { "a" } else { "span" });
            let t = dom.new_text(&title);
            dom.append_child(a, t);
            dom.append_child(li, a);
            if let Some(dest) = toc.node(child).dest.clone() {
                let mut href = name_to_href_at(&dest, root_path, Some(tocname));
                if let Some(frag) = &toc.node(child).frag {
                    href.push('#');
                    href.push_str(frag);
                }
                dom.node_mut(a).attrs.insert("href".to_string(), href);
            }
            if !toc.children(child).is_empty() {
                let ol = dom.new_element("ol");
                dom.append_child(li, ol);
                process_node(dom, toc, ol, child, root_path, tocname);
            }
        }
    }
    process_node(dom, toc, rnode, toc.root, &root_path, &tocname);
    pretty_dom_xml_tree(dom, nav, 0, "  ");
    collapse_li_dom(dom, nav);
    set_dom_tail(dom, nav, "\n");

    if !toc.page_list.is_empty() {
        let nav2 = ensure_single_nav_of_type(dom, "page-list");
        dom.node_mut(nav2)
            .attrs
            .insert("hidden".to_string(), String::new());
        let ol = dom.new_element("ol");
        dom.append_child(nav2, ol);
        for (dest, frag, pagenum) in &valid_page_list {
            let a = create_nav_li_dom(dom, ol, dest, frag.as_deref(), &root_path, &tocname);
            let t = dom.new_text(pagenum);
            dom.append_child(a, t);
        }
        pretty_dom_xml_tree(dom, nav2, 0, "  ");
        collapse_li_dom(dom, nav2);
    }

    container.dirty(&tocname);
    Ok(())
}

/// Port of `set_landmarks`.
pub fn set_landmarks(
    container: &mut Container,
    tocname: &str,
    landmarks: &[Landmark],
) -> Result<()> {
    let root_path = container.root.clone();
    let valid: Vec<Landmark> = landmarks
        .iter()
        .filter(|e| {
            !e.r#type.is_empty()
                && container.has_name(&e.dest)
                && container
                    .base
                    .mime_map
                    .get(&e.dest)
                    .map(|m| OEB_DOCS.contains(&m.as_str()))
                    .unwrap_or(false)
        })
        .cloned()
        .collect();

    container.ensure_parsed(tocname)?;
    let dom = container.get_xhtml_mut(tocname)?;
    let nav = ensure_single_nav_of_type(dom, "landmarks");
    dom.node_mut(nav)
        .attrs
        .insert("hidden".to_string(), String::new());
    let ol = dom.new_element("ol");
    dom.append_child(nav, ol);
    for entry in &valid {
        let frag = if entry.frag.is_empty() {
            None
        } else {
            Some(entry.frag.as_str())
        };
        let a = create_nav_li_dom(dom, ol, &entry.dest, frag, &root_path, tocname);
        dom.node_mut(a)
            .attrs
            .insert("epub:type".to_string(), entry.r#type.clone());
        if !entry.title.is_empty() {
            let t = dom.new_text(&entry.title);
            dom.append_child(a, t);
        }
    }
    pretty_dom_xml_tree(dom, nav, 0, "  ");
    collapse_li_dom(dom, nav);
    container.dirty(tocname);
    Ok(())
}

/// Port of `commit_toc`.
pub fn commit_toc(
    container: &mut Container,
    toc: &Toc,
    lang: Option<&str>,
    uid: Option<&str>,
) -> Result<()> {
    commit_ncx_toc(container, toc, lang, uid)?;
    if container.opf_version_parsed()?.0 > 2 {
        commit_nav_toc(container, toc, lang, None, None)?;
    }
    Ok(())
}

/// Port of `remove_names_from_toc`. Python's shared loop over
/// `((find_existing_ncx_toc, parse_ncx, commit_ncx_toc),
/// (find_existing_nav_toc, parse_nav, commit_nav_toc))` binds `commit_toc`
/// to a different function each iteration; `commit_ncx_toc`/
/// `commit_nav_toc` have different Rust signatures (the latter takes
/// `landmarks`/`previous_nav`), so the two iterations are unrolled here
/// instead of sharing one generic closure parameter.
pub fn remove_names_from_toc(
    container: &mut Container,
    names: &HashSet<String>,
) -> Result<Vec<Option<String>>> {
    let mut changed = Vec::new();

    let mut ncx_toc = get_x_toc(container, find_existing_ncx_toc, parse_ncx, false)?;
    if !ncx_toc.is_empty(ncx_toc.root) {
        let remove: Vec<TocNodeId> = ncx_toc
            .iterdescendants(ncx_toc.root)
            .into_iter()
            .filter(|&n| {
                ncx_toc
                    .node(n)
                    .dest
                    .as_ref()
                    .map(|d| names.contains(d))
                    .unwrap_or(false)
            })
            .collect();
        if !remove.is_empty() {
            for &node in remove.iter().rev() {
                ncx_toc.remove_from_parent(node);
            }
            commit_ncx_toc(container, &ncx_toc, None, None)?;
            changed.push(find_existing_ncx_toc(container)?);
        }
    }

    let mut nav_toc = get_x_toc(container, find_existing_nav_toc, parse_nav, false)?;
    if !nav_toc.is_empty(nav_toc.root) {
        let remove: Vec<TocNodeId> = nav_toc
            .iterdescendants(nav_toc.root)
            .into_iter()
            .filter(|&n| {
                nav_toc
                    .node(n)
                    .dest
                    .as_ref()
                    .map(|d| names.contains(d))
                    .unwrap_or(false)
            })
            .collect();
        if !remove.is_empty() {
            for &node in remove.iter().rev() {
                nav_toc.remove_from_parent(node);
            }
            commit_nav_toc(container, &nav_toc, None, None, None)?;
            changed.push(find_existing_nav_toc(container)?);
        }
    }

    Ok(changed)
}

// ===================================================================
// Inline (HTML) TOC
// ===================================================================

/// Port of `find_inline_toc`.
pub fn find_inline_toc(container: &mut Container) -> Result<Option<String>> {
    let spine = container.spine_names()?;
    for (name, _linear) in spine {
        container.ensure_parsed(&name)?;
        let dom = container.get_xhtml(&name)?;
        if let Some(body) = dom.find_first_tag_global("body") {
            if dom.node(body).attrs.get("id").map(|s| s.as_str())
                == Some("calibre_generated_inline_toc")
            {
                return Ok(Some(name));
            }
        }
    }
    Ok(None)
}

/// Port of `toc_to_html`.
///
/// The generated `<style>` tag is left empty until *after*
/// [`pretty_html_tree`] runs, then filled in directly: `pretty_html_tree`
/// would otherwise call `pretty.rs`'s still-unimplemented
/// `pretty_css_text` for any `<style>` tag that already has text content
/// (a documented gap inherited from that module, see its docs) --
/// ordering the insertion this way sidesteps that entirely, since the
/// CSS here is a small, already-well-formatted static asset that never
/// needed reformatting in the first place.
pub fn toc_to_html(
    toc: &Toc,
    root_path: &Path,
    toc_name: Option<&str>,
    title: &str,
    lang: Option<&str>,
) -> Result<Dom> {
    let mut dom = Dom::parse("<html><head></head><body></body></html>");
    let html = dom
        .find_first_tag_global("html")
        .expect("html5ever always synthesizes <html>");
    let head = dom
        .find_first_tag_global("head")
        .expect("html5ever always synthesizes <head>");
    let body = dom
        .find_first_tag_global("body")
        .expect("html5ever always synthesizes <body>");

    let title_el = dom.new_element("title");
    let t = dom.new_text(title);
    dom.append_child(title_el, t);
    dom.append_child(head, title_el);

    let style_el = dom.new_element("style");
    dom.node_mut(style_el)
        .attrs
        .insert("type".to_string(), "text/css".to_string());
    dom.append_child(head, style_el);

    let h2 = dom.new_element("h2");
    let h2t = dom.new_text(title);
    dom.append_child(h2, h2t);
    dom.append_child(body, h2);

    let ul = dom.new_element("ul");
    dom.node_mut(ul)
        .attrs
        .insert("class".to_string(), "level1".to_string());
    dom.append_child(body, ul);
    dom.node_mut(body)
        .attrs
        .insert("id".to_string(), "calibre_generated_inline_toc".to_string());

    #[allow(clippy::too_many_arguments)]
    fn process_node(
        dom: &mut Dom,
        toc: &Toc,
        html_parent: NodeId,
        node: TocNodeId,
        root_path: &Path,
        toc_name: Option<&str>,
        level: usize,
        indent: &str,
        style_level: usize,
    ) {
        let li = dom.new_element("li");
        dom.append_child(html_parent, li);
        set_dom_tail(dom, li, &format!("\n{}", indent.repeat(level)));

        let name = toc.node(node).dest.clone();
        let frag = toc.node(node).frag.clone();
        let mut href = "#".to_string();
        if let Some(name) = &name {
            href = name_to_href_at(name, root_path, toc_name);
            if let Some(f) = &frag {
                href.push('#');
                href.push_str(f);
            }
        }
        let a = dom.new_element("a");
        dom.node_mut(a).attrs.insert("href".to_string(), href);
        if let Some(t) = &toc.node(node).title {
            let tn = dom.new_text(t);
            dom.append_child(a, tn);
        }
        dom.append_child(li, a);

        let children = toc.children(node).to_vec();
        if !children.is_empty() {
            let parent = dom.new_element("ul");
            dom.node_mut(parent)
                .attrs
                .insert("class".to_string(), format!("level{style_level}"));
            dom.append_child(li, parent);
            set_dom_tail(dom, a, &format!("\n\n{}", indent.repeat(level + 2)));
            set_dom_leading_text(dom, parent, &format!("\n{}", indent.repeat(level + 3)));
            set_dom_tail(dom, parent, &format!("\n\n{}", indent.repeat(level + 1)));
            for &child in &children {
                process_node(
                    dom,
                    toc,
                    parent,
                    child,
                    root_path,
                    toc_name,
                    level + 3,
                    indent,
                    style_level + 1,
                );
            }
            if let Some(&last) = children.last() {
                set_dom_tail(dom, last, &format!("\n{}", indent.repeat(level + 2)));
            }
        }
    }

    for &child in toc.children(toc.root) {
        process_node(&mut dom, toc, ul, child, root_path, toc_name, 1, "  ", 2);
    }

    if let Some(lang) = lang {
        dom.node_mut(html)
            .attrs
            .insert("lang".to_string(), lang.to_string());
    }

    pretty_html_tree(&mut dom)?;

    let css_text = dom.new_text(INLINE_TOC_STYLES_CSS);
    dom.insert_child(style_el, 0, css_text);

    Ok(dom)
}

/// Port of `create_inline_toc`.
///
/// `translate(lang, default_title)` in the original calls calibre's
/// full runtime i18n system (`calibre.translations.dynamic`), entirely
/// out of scope for this port (see the module docs); the English source
/// string `"Table of Contents"` is used as an identity passthrough
/// regardless of `lang`, matching how every other `_('...')`-style call
/// has been handled throughout this project.
pub fn create_inline_toc(container: &mut Container, title: Option<&str>) -> Result<Option<String>> {
    let lang = super::opf::get_book_language(container)?;
    let resolved_lang = lang.map(|l| lang_as_iso639_1(&l).unwrap_or(l));

    let title = title
        .map(|s| s.to_string())
        .unwrap_or_else(|| "Table of Contents".to_string());
    let toc = get_toc(container, true)?;
    if toc.is_empty(toc.root) {
        return Ok(None);
    }
    let toc_name = find_inline_toc(container)?;

    let root_path = container.root.clone();
    let dom = toc_to_html(
        &toc,
        &root_path,
        toc_name.as_deref(),
        &title,
        resolved_lang.as_deref(),
    )?;
    let raw = dom.serialize(dom.root).into_bytes();

    let name = match toc_name {
        Some(name) => {
            container.write_file(&name, &raw)?;
            name
        }
        None => {
            let mut name = "toc.xhtml".to_string();
            let mut c = 0u32;
            while container.has_name(&name) {
                c += 1;
                name = format!("toc{c}.xhtml");
            }
            container.add_file(&name, &raw, None, Some(0), false)?
        }
    };
    super::opf::set_guide_item(
        container,
        "toc",
        &title,
        Some(&name),
        Some("calibre_generated_inline_toc"),
    )?;
    Ok(Some(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_v2_book(dir: &Path) {
        fs::write(
            dir.join("content.opf"),
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata>
    <dc:title>Test Book</dc:title>
    <dc:language>en</dc:language>
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
  <guide>
    <reference type="cover" title="Cover" href="chap1.html"/>
  </guide>
</package>"#,
        )
        .unwrap();
        fs::write(
            dir.join("chap1.html"),
            b"<html><body><h1 id=\"top\">Chapter One</h1><p>hello <a href=\"chap2.html#s2\">link</a></p></body></html>",
        )
        .unwrap();
        fs::write(
            dir.join("chap2.html"),
            b"<html><body><h1>Chapter Two</h1><h2 id=\"s2\">Section</h2><p>world</p></body></html>",
        )
        .unwrap();
    }

    fn write_v3_book(dir: &Path) {
        fs::write(
            dir.join("content.opf"),
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="3.0" unique-identifier="bookid">
  <metadata>
    <dc:title>Test Book V3</dc:title>
    <dc:language>en</dc:language>
    <dc:identifier id="bookid">urn:uuid:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee</dc:identifier>
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
            b"<html><body><h1 id=\"top\">Chapter One</h1><p>hello <a href=\"chap2.html#s2\">link</a></p></body></html>",
        )
        .unwrap();
        fs::write(
            dir.join("chap2.html"),
            b"<html><body><h1>Chapter Two</h1><h2 id=\"s2\">Section</h2><p>world</p></body></html>",
        )
        .unwrap();
    }

    fn sample_toc() -> Toc {
        let mut toc = Toc::new();
        let c1 = toc.add(
            toc.root,
            Some("Chapter One".into()),
            Some("chap1.html".into()),
            None,
        );
        toc.add(
            c1,
            Some("Section 1.1".into()),
            Some("chap1.html".into()),
            Some("s1".into()),
        );
        toc.add(
            toc.root,
            Some("Chapter Two".into()),
            Some("chap2.html".into()),
            Some("s2".into()),
        );
        toc
    }

    #[test]
    fn toc_tree_add_and_shape() {
        let toc = sample_toc();
        assert_eq!(toc.len(toc.root), 2);
        // root -> Chapter One -> Section 1.1 (3 levels deep), and
        // root -> Chapter Two (2 levels) -- depth is the deeper branch.
        assert_eq!(toc.depth(toc.root), 3);
        let c1 = toc.children(toc.root)[0];
        assert_eq!(toc.node(c1).title.as_deref(), Some("Chapter One"));
        assert_eq!(toc.len(c1), 1);
        assert_eq!(toc.last_child(toc.root), Some(toc.children(toc.root)[1]));
    }

    #[test]
    fn toc_iterdescendants_and_display() {
        let toc = sample_toc();
        let all = toc.iterdescendants(toc.root);
        assert_eq!(all.len(), 3);
        let s = toc.to_string();
        assert!(s.contains("Chapter One"));
        assert!(s.contains("Section 1.1"));
    }

    #[test]
    fn toc_remove_from_parent_promotes_children() {
        let mut toc = sample_toc();
        let root_children_before = toc.children(toc.root).to_vec();
        let c1 = root_children_before[0];
        let second = root_children_before[1];
        let sub = toc.children(c1)[0];
        toc.remove_from_parent(c1);
        assert_eq!(toc.children(toc.root), &[sub, second]);
        assert_eq!(toc.parent(sub), Some(toc.root));
        assert_eq!(toc.parent(c1), None);
    }

    #[test]
    fn toc_remove_duplicates_by_title() {
        let mut toc = Toc::new();
        toc.add(toc.root, Some("A".into()), Some("a.html".into()), None);
        toc.add(toc.root, Some("A".into()), Some("b.html".into()), None);
        toc.add(toc.root, Some("B".into()), Some("c.html".into()), None);
        toc.remove_duplicates(toc.root, true);
        assert_eq!(toc.len(toc.root), 2);
    }

    #[test]
    fn ncx_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        write_v2_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let toc = sample_toc();
        commit_ncx_toc(&mut c, &toc, None, None).unwrap();
        c.commit(false).unwrap();

        let mut c2 = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let tocname = find_existing_ncx_toc(&mut c2).unwrap().unwrap();
        assert_eq!(tocname, "toc.ncx");
        let parsed = parse_ncx(&mut c2, &tocname).unwrap();
        assert_eq!(parsed.len(parsed.root), 2);
        let c1 = parsed.children(parsed.root)[0];
        assert_eq!(parsed.node(c1).title.as_deref(), Some("Chapter One"));
        assert_eq!(parsed.node(c1).dest.as_deref(), Some("chap1.html"));
        assert_eq!(parsed.len(c1), 1);
        let sub = parsed.children(c1)[0];
        assert_eq!(parsed.node(sub).frag.as_deref(), Some("s1"));
    }

    #[test]
    fn nav_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        write_v3_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let toc = sample_toc();
        commit_nav_toc(&mut c, &toc, None, None, None).unwrap();
        c.commit(false).unwrap();

        let mut c2 = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let tocname = find_existing_nav_toc(&mut c2).unwrap().unwrap();
        let parsed = parse_nav(&mut c2, &tocname).unwrap();
        assert_eq!(parsed.len(parsed.root), 2);
        let c1 = parsed.children(parsed.root)[0];
        assert_eq!(parsed.node(c1).title.as_deref(), Some("Chapter One"));
        assert_eq!(parsed.len(c1), 1);
    }

    #[test]
    fn get_toc_dispatches_by_opf_version() {
        let dir = tempfile::tempdir().unwrap();
        write_v2_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let toc = get_toc(&mut c, true).unwrap();
        assert!(toc.is_empty(toc.root));
    }

    #[test]
    fn commit_toc_writes_ncx_and_nav_for_v3() {
        let dir = tempfile::tempdir().unwrap();
        write_v3_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let toc = sample_toc();
        commit_toc(&mut c, &toc, None, None).unwrap();
        c.commit(false).unwrap();
        assert!(dir.path().join("toc.ncx").exists());
        let mut c2 = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let got = get_toc(&mut c2, true).unwrap();
        assert_eq!(got.len(got.root), 2);
    }

    #[test]
    fn from_files_builds_one_entry_per_spine_item() {
        let dir = tempfile::tempdir().unwrap();
        write_v2_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let toc = from_files(&mut c).unwrap();
        assert_eq!(toc.len(toc.root), 2);
        assert_eq!(
            toc.node(toc.children(toc.root)[0]).title.as_deref(),
            Some("Chapter One")
        );
    }

    #[test]
    fn from_links_dedupes_and_verifies_destinations() {
        let dir = tempfile::tempdir().unwrap();
        write_v2_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let toc = from_links(&mut c).unwrap();
        assert_eq!(toc.len(toc.root), 1);
        let link = toc.children(toc.root)[0];
        assert_eq!(toc.node(link).dest.as_deref(), Some("chap2.html"));
        assert_eq!(toc.node(link).frag.as_deref(), Some("s2"));
    }

    #[test]
    fn from_xpaths_builds_hierarchy_from_headings() {
        let dir = tempfile::tempdir().unwrap();
        write_v2_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let toc = from_xpaths(&mut c, &["//h:h1", "//h:h2"], false).unwrap();
        assert_eq!(toc.len(toc.root), 2);
        let ch2 = toc.children(toc.root)[1];
        assert_eq!(toc.len(ch2), 1);
    }

    #[test]
    fn get_guide_landmarks_reads_opf_guide() {
        let dir = tempfile::tempdir().unwrap();
        write_v2_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let lm = get_guide_landmarks(&mut c).unwrap();
        assert_eq!(lm.len(), 1);
        assert_eq!(lm[0].dest, "chap1.html");
        assert_eq!(lm[0].r#type, "cover");
    }

    #[test]
    fn set_landmarks_and_get_nav_landmarks_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        write_v3_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let tocname = ensure_container_has_nav(&mut c, None, None).unwrap();
        set_landmarks(
            &mut c,
            &tocname,
            &[Landmark {
                dest: "chap1.html".into(),
                frag: String::new(),
                title: "Start".into(),
                r#type: "bodymatter".into(),
            }],
        )
        .unwrap();
        c.commit(false).unwrap();

        let mut c2 = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let lm = get_nav_landmarks(&mut c2).unwrap();
        assert_eq!(lm.len(), 1);
        assert_eq!(lm[0].dest, "chap1.html");
        assert_eq!(lm[0].r#type, "bodymatter");
    }

    #[test]
    fn verify_toc_destinations_flags_missing_and_bad_frag() {
        let dir = tempfile::tempdir().unwrap();
        write_v2_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let mut toc = Toc::new();
        toc.add(
            toc.root,
            Some("Good".into()),
            Some("chap1.html".into()),
            Some("top".into()),
        );
        toc.add(
            toc.root,
            Some("BadFrag".into()),
            Some("chap1.html".into()),
            Some("nope".into()),
        );
        toc.add(
            toc.root,
            Some("BadFile".into()),
            Some("missing.html".into()),
            None,
        );
        verify_toc_destinations(&mut c, &mut toc).unwrap();
        let kids = toc.children(toc.root).to_vec();
        assert_eq!(toc.node(kids[0]).dest_exists, Some(true));
        assert_eq!(toc.node(kids[1]).dest_exists, Some(false));
        assert_eq!(toc.node(kids[2]).dest_exists, Some(false));
    }

    #[test]
    fn remove_names_from_toc_prunes_matching_entries() {
        let dir = tempfile::tempdir().unwrap();
        write_v2_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let toc = sample_toc();
        commit_ncx_toc(&mut c, &toc, None, None).unwrap();
        c.commit(false).unwrap();

        let mut c2 = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let mut names = HashSet::new();
        names.insert("chap2.html".to_string());
        let changed = remove_names_from_toc(&mut c2, &names).unwrap();
        assert!(!changed.is_empty());
        c2.commit(false).unwrap();

        let mut c3 = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let tocname = find_existing_ncx_toc(&mut c3).unwrap().unwrap();
        let parsed = parse_ncx(&mut c3, &tocname).unwrap();
        for id in parsed.iterdescendants(parsed.root) {
            assert_ne!(parsed.node(id).dest.as_deref(), Some("chap2.html"));
        }
    }

    #[test]
    fn create_inline_toc_generates_html_page() {
        let dir = tempfile::tempdir().unwrap();
        write_v2_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let toc = sample_toc();
        commit_ncx_toc(&mut c, &toc, None, None).unwrap();
        c.commit(false).unwrap();

        let mut c2 = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let name = create_inline_toc(&mut c2, None).unwrap().unwrap();
        c2.commit(false).unwrap();
        assert!(c2.has_name(&name));
        let raw = fs::read_to_string(dir.path().join(&name)).unwrap();
        assert!(raw.contains("Chapter One"));
        assert!(raw.contains("calibre_generated_inline_toc"));
    }

    #[test]
    fn item_at_top_true_only_for_first_content() {
        let dom = Dom::parse("<html><body><h1 id=\"a\">Title</h1><p>text</p></body></html>");
        let h1 = dom.find_first_tag_global("h1").unwrap();
        assert!(item_at_top(&dom, h1));
        let p = dom.find_first_tag_global("p").unwrap();
        assert!(!item_at_top(&dom, p));
    }
}
