//! Port of `old_src/src/calibre/ebooks/docx/toc.py`.
//!
//! Builds a table of contents for a converted DOCX document, preferring
//! a Word-generated `TOC` field (`from_toc`, walking the *source*
//! `w:fldChar`/`w:hyperlink`/`w:instrText` elements) and falling back to
//! one synthesized from heading elements (`from_headings`, walking the
//! *converted* HTML tree for `data-heading-level`-tagged elements —
//! [`super::to_html::convert_p`] stamps that attribute already).
//!
//! # The [`Toc`] tree
//!
//! Python's `calibre.ebooks.metadata.toc.TOC` is a self-referential
//! `list` subclass (each node is its own `TOC` instance, holding a
//! `parent` back-reference and doubling as its own children list) --
//! ported here as an arena (`Toc` owning a flat `Vec<TocNode>`, nodes
//! referenced by [`TocNodeId`]), the same shape
//! [`crate::oeb::polish::toc::Toc`] uses for the unrelated,
//! differently-shaped `oeb.polish.toc.TOC` class. **Do not conflate the
//! two** -- this `Toc` only has `href`/`fragment`/`text`/`play_order`,
//! matching `calibre.ebooks.metadata.toc.TOC`'s actual fields, not that
//! module's landmark/dest-verification concerns.
//!
//! `log(...)` calls (Python logs which TOC-generation strategy was
//! used) are dropped rather than logged, matching the precedent set by
//! [`super::to_html::resolve_links`]: no logger is threaded through
//! this module yet.

use std::collections::HashMap;

use indexmap::IndexMap;
use roxmltree::Node;

use crate::dom::{Dom, NodeId};
use crate::oeb::polish::toc::elem_to_toc_text;

use super::fonts::Fonts;
use super::names::DocxNamespace;
use super::styles::Styles;
use super::theme::Theme;

pub type TocNodeId = usize;

/// One node of a [`Toc`] tree. Port of a `TOC` instance's fields that
/// this module actually uses (`href`/`fragment`/`text`/`play_order`).
#[derive(Debug, Clone, Default)]
pub struct TocNode {
    pub href: Option<String>,
    pub fragment: Option<String>,
    pub text: Option<String>,
    pub play_order: i32,
    pub children: Vec<TocNodeId>,
}

/// Port of `calibre.ebooks.metadata.toc.TOC`. See the module docs for
/// the arena design and why this is unrelated to
/// [`crate::oeb::polish::toc::Toc`].
#[derive(Debug, Clone)]
pub struct Toc {
    nodes: Vec<TocNode>,
    /// The synthetic root `TOC()` instance every ported function
    /// builds; its own `href`/`fragment`/`text` stay `None`.
    pub root: TocNodeId,
}

impl Default for Toc {
    fn default() -> Self {
        Self::new()
    }
}

impl Toc {
    pub fn new() -> Self {
        Toc {
            nodes: vec![TocNode::default()],
            root: 0,
        }
    }

    pub fn node(&self, id: TocNodeId) -> &TocNode {
        &self.nodes[id]
    }

    pub fn children(&self, id: TocNodeId) -> &[TocNodeId] {
        &self.nodes[id].children
    }

    /// Port of `TOC.add_item`: appends a new child of `parent`, its
    /// `play_order` one past `parent`'s last child (or `parent`'s own
    /// `play_order` if it has none yet).
    pub fn add_item(
        &mut self,
        parent: TocNodeId,
        href: &str,
        fragment: &str,
        text: &str,
    ) -> TocNodeId {
        let play_order = self.nodes[parent]
            .children
            .last()
            .map(|&c| self.nodes[c].play_order)
            .unwrap_or(self.nodes[parent].play_order)
            + 1;
        let id = self.nodes.len();
        self.nodes.push(TocNode {
            href: Some(href.to_string()),
            fragment: Some(fragment.to_string()).filter(|f| !f.is_empty()),
            text: Some(text.to_string()),
            play_order,
            children: Vec::new(),
        });
        self.nodes[parent].children.push(id);
        id
    }

    /// Port of `len(tuple(toc.flat()))`: `id` plus every descendant,
    /// depth-first.
    fn flat_count(&self, id: TocNodeId) -> usize {
        1 + self.nodes[id]
            .children
            .iter()
            .map(|&c| self.flat_count(c))
            .sum::<usize>()
    }

    /// Converts this arena into `crate::metadata::toc::TOC`'s
    /// tree-of-`TOCNode` shape -- what `crate::opf_writer::write_ncx`
    /// needs to render a real `toc.ncx` (issue #130/#288's `write`
    /// step). Not a general-purpose conversion: it's specific to what
    /// this file's own `Toc` ever actually contains (every node's
    /// `href` is always `"index.html"`, from `from_headings`/
    /// `structure_toc`'s own `add_item` calls) -- `href`/`fragment`
    /// combine into a single `"href#fragment"` `src` (or just `href`
    /// with no fragment), `text` becomes `title`.
    pub fn to_ncx_toc(&self) -> crate::metadata::toc::TOC {
        fn convert(toc: &Toc, id: TocNodeId) -> Vec<crate::metadata::toc::TOCNode> {
            toc.children(id)
                .iter()
                .map(|&child_id| {
                    let node = toc.node(child_id);
                    let href = node.href.as_deref().unwrap_or("index.html");
                    let src = match node.fragment.as_deref() {
                        Some(frag) if !frag.is_empty() => format!("{href}#{frag}"),
                        _ => href.to_string(),
                    };
                    crate::metadata::toc::TOCNode {
                        title: node.text.clone().unwrap_or_default(),
                        src,
                        children: convert(toc, child_id),
                    }
                })
                .collect()
        }
        crate::metadata::toc::TOC {
            nodes: convert(self, self.root),
        }
    }
}

/// Create a TOC from `data-heading-level`-tagged elements in the
/// converted HTML `body`. Assigns each heading an `id` (reusing an
/// existing one, else generating `toc_id_N`) and nests entries by
/// heading level, walking up to the nearest ancestor level already
/// seen when a level is skipped (e.g. an `h3` with no preceding `h2`
/// attaches directly under the current `h1`, or the root).
///
/// Returns `None` when fewer than 2 entries were found (Python's
/// `len(tuple(tocroot.flat())) > 1` -- `flat()` always includes the
/// root itself, so this really means "at least one real heading").
///
/// Port of `from_headings`.
pub fn from_headings(dom: &mut Dom, body: NodeId, num_levels: i32) -> Option<Toc> {
    let heading_nodes: Vec<NodeId> = dom
        .preorder_elements(body)
        .into_iter()
        .filter(|&n| dom.node(n).attrs.contains_key("data-heading-level"))
        .collect();

    let mut toc = Toc::new();
    let mut level_prev: HashMap<i32, Option<TocNodeId>> =
        (0..=num_levels).map(|i| (i, None)).collect();
    level_prev.insert(0, Some(toc.root));

    let item_level: HashMap<NodeId, i32> = heading_nodes
        .iter()
        .filter_map(|&n| {
            let lvl: i32 = dom.node(n).attrs.get("data-heading-level")?.parse().ok()?;
            (1..=num_levels).contains(&lvl).then_some((n, lvl))
        })
        .collect();

    let mut next_id = 0u32;
    for item in heading_nodes {
        let Some(&item_lvl) = item_level.get(&item) else {
            continue;
        };
        let mut plvl = item_lvl;
        let parent = loop {
            plvl -= 1;
            if let Some(p) = level_prev.get(&plvl).copied().flatten() {
                break p;
            }
        };
        let lvl = plvl + 1;

        let elem_id = match dom.node(item).attrs.get("id") {
            Some(id) if !id.is_empty() => id.clone(),
            _ => {
                next_id += 1;
                let id = format!("toc_id_{next_id}");
                dom.node_mut(item)
                    .attrs
                    .insert("id".to_string(), id.clone());
                id
            }
        };
        let text = elem_to_toc_text(dom, item, false);

        let node = toc.add_item(parent, "index.html", &elem_id, &text);
        level_prev.insert(lvl, Some(node));
        for i in (lvl + 1)..=num_levels {
            level_prev.insert(i, None);
        }
    }

    (toc.flat_count(toc.root) > 1).then_some(toc)
}

/// `a`'s text content, first detaching any direct child whose resolved
/// style is `display: none` (Word sometimes leaves invisible runs
/// inside a TOC hyperlink -- a hidden page-number tab leader, say).
/// `a`'s children are converted `w:r`/`w:p` spans, each a key in
/// `object_map`; `styles.resolve(...)`'s generic paragraph-vs-run
/// dispatch is inlined the same way [`super::to_html::assign_style_classes`]
/// does, for the same reason (no common Rust return type without an
/// enum wrapper solely for this).
///
/// Port of `link_to_txt`.
fn link_to_txt<'a, 'i>(
    dom: &mut Dom,
    a: NodeId,
    styles: &mut Styles<'a, 'i>,
    theme: &Theme,
    fonts: &mut Fonts,
    object_map: &IndexMap<NodeId, Node<'a, 'i>>,
    ns: &DocxNamespace,
) -> String {
    let children = dom.children(a);
    if children.len() > 1 {
        for child in children {
            let Some(&run) = object_map.get(&child) else {
                continue;
            };
            let css = if ns.is_tag(run, "w:p") {
                styles.resolve_paragraph(run, ns).css()
            } else if ns.is_tag(run, "w:r") {
                styles.resolve_run(run, theme, fonts, ns).css()
            } else {
                continue;
            };
            if css.get("display").map(String::as_str) == Some("none") {
                dom.detach(child);
            }
        }
    }
    dom.text_content(a).trim().to_string()
}

struct TocEntry {
    text: String,
    anchor: String,
    indent: f64,
}

/// Create a TOC from a Word-generated `TOC` field: walks the *source*
/// document for `w:fldChar`/`w:hyperlink`/`w:instrText`, tracking field
/// nesting `level` via `w:fldChar[@w:fldCharType]` begin/end pairs, and
/// noting `toc_level` once a `TOC ` instruction is seen inside the
/// current field. Every `w:hyperlink` seen at or below `toc_level`
/// while still inside that field becomes one entry, keyed by its
/// resolved paragraph's `margin_left` (used as an indent level -- reset
/// to 0 for centered/right-aligned paragraphs, matching Python).
///
/// `resolved_link_map` is Python's `self.resolved_link_map` (the
/// `w:hyperlink -> <a>` map [`super::to_html::resolve_links`]
/// returns) -- despite the parameter being named `link_map` in the
/// Python source, it is not `self.link_map` (the earlier
/// `w:hyperlink -> [<span>, ...]` map).
///
/// Port of `from_toc`.
pub fn from_toc<'a, 'i>(
    document: Node<'a, 'i>,
    resolved_link_map: &HashMap<Node<'a, 'i>, NodeId>,
    dom: &mut Dom,
    styles: &mut Styles<'a, 'i>,
    theme: &Theme,
    fonts: &mut Fonts,
    object_map: &IndexMap<NodeId, Node<'a, 'i>>,
    ns: &DocxNamespace,
) -> Option<Toc> {
    let mut toc_level: Option<i32> = None;
    let mut level = 0i32;
    let mut entries: Vec<TocEntry> = Vec::new();

    for tag in ns.descendants(document, &["w:fldChar", "w:hyperlink", "w:instrText"]) {
        if ns.is_tag(tag, "w:fldChar") {
            match ns.get(tag, "w:fldCharType") {
                Some("begin") => level += 1,
                Some("end") => {
                    level -= 1;
                    if toc_level.is_some_and(|tl| level < tl) {
                        break;
                    }
                }
                _ => {}
            }
        } else if ns.is_tag(tag, "w:instrText") {
            if level > 0 {
                if let Some(text) = tag.text() {
                    if text.trim_start().starts_with("TOC ") {
                        toc_level = Some(level);
                    }
                }
            }
        } else if ns.is_tag(tag, "w:hyperlink") {
            let Some(tl) = toc_level else { continue };
            if level < tl {
                continue;
            }
            let Some(&a) = resolved_link_map.get(&tag) else {
                continue;
            };
            let href = dom.node(a).attrs.get("href").cloned();
            let txt = link_to_txt(dom, a, styles, theme, fonts, object_map, ns);
            let p = ns.ancestor(tag, "w:p");
            let (Some(href), Some(p)) = (href, p) else {
                continue;
            };
            if txt.is_empty() {
                continue;
            }
            let ps = styles.resolve_paragraph(p, ns);
            let mut ml = ps
                .margin_left
                .as_deref()
                .and_then(|m| m.strip_suffix("pt"))
                .and_then(|m| m.parse::<f64>().ok())
                .unwrap_or(0.0);
            if matches!(ps.text_align.as_deref(), Some("center") | Some("right")) {
                ml = 0.0;
            }
            entries.push(TocEntry {
                text: txt,
                anchor: href[1..].to_string(),
                indent: ml,
            });
        }
    }

    (!entries.is_empty()).then(|| structure_toc(entries))
}

/// Nests flat `(text, anchor, indent)` entries into a tree by distinct
/// indent value (sorted ascending, each one a nesting level) -- unless
/// there are more than 6 distinct indents, in which case Word's
/// indentation is deemed unreliable and every entry becomes a
/// top-level item instead. An entry whose own level has no open parent
/// yet climbs to the nearest shallower level still open, or the root.
///
/// Port of `structure_toc`.
fn structure_toc(entries: Vec<TocEntry>) -> Toc {
    let mut indent_vals: Vec<f64> = entries.iter().map(|e| e.indent).collect();
    indent_vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    indent_vals.dedup();

    let mut toc = Toc::new();

    if indent_vals.len() > 6 {
        for e in &entries {
            toc.add_item(toc.root, "index.html", &e.anchor, &e.text);
        }
        return toc;
    }

    let mut last_found: Vec<Option<TocNodeId>> = vec![None; indent_vals.len()];
    for e in &entries {
        let level = indent_vals.iter().position(|&v| v == e.indent).unwrap();
        let parent = last_found[..level]
            .iter()
            .rev()
            .find_map(|&x| x)
            .unwrap_or(toc.root);
        let node = toc.add_item(parent, "index.html", &e.anchor, &e.text);
        last_found[level] = Some(node);
        for slot in last_found.iter_mut().skip(level + 1) {
            *slot = None;
        }
    }

    toc
}

/// `from_toc`, falling back to `from_headings` (with Python's default
/// `num_levels=3`) when no Word-generated TOC field was found. Either
/// way, strips every `data-heading-level` attribute [`super::to_html::convert_p`]
/// left behind -- it was only ever a marker for this pass.
///
/// Port of `create_toc`.
pub fn create_toc<'a, 'i>(
    document: Node<'a, 'i>,
    dom: &mut Dom,
    body: NodeId,
    resolved_link_map: &HashMap<Node<'a, 'i>, NodeId>,
    styles: &mut Styles<'a, 'i>,
    theme: &Theme,
    fonts: &mut Fonts,
    object_map: &IndexMap<NodeId, Node<'a, 'i>>,
    ns: &DocxNamespace,
) -> Option<Toc> {
    let ans = from_toc(
        document,
        resolved_link_map,
        dom,
        styles,
        theme,
        fonts,
        object_map,
        ns,
    )
    .or_else(|| from_headings(dom, body, 3));

    for h in dom.preorder_elements(body) {
        dom.node_mut(h).attrs.shift_remove("data-heading-level");
    }

    ans
}

#[cfg(test)]
mod from_headings_tests {
    use super::*;

    fn heading(dom: &mut Dom, parent: NodeId, level: &str, text: &str) -> NodeId {
        let h = dom.new_element("p");
        dom.node_mut(h)
            .attrs
            .insert("data-heading-level".to_string(), level.to_string());
        let t = dom.new_text(text);
        dom.append_child(h, t);
        dom.append_child(parent, h);
        h
    }

    #[test]
    fn a_single_heading_still_yields_a_toc() {
        // `flat()` counts the root plus every descendant, so even one
        // heading gives a count of 2 (root + itself) -- `> 1` passes.
        let mut dom = Dom::empty();
        let body = dom.new_element("body");
        heading(&mut dom, body, "1", "Chapter One");

        let toc = from_headings(&mut dom, body, 3).unwrap();
        assert_eq!(toc.children(toc.root).len(), 1);
    }

    #[test]
    fn no_headings_yields_no_toc() {
        let mut dom = Dom::empty();
        let body = dom.new_element("body");

        assert!(from_headings(&mut dom, body, 3).is_none());
    }

    #[test]
    fn two_top_level_headings_nest_flat() {
        let mut dom = Dom::empty();
        let body = dom.new_element("body");
        heading(&mut dom, body, "1", "One");
        heading(&mut dom, body, "1", "Two");

        let toc = from_headings(&mut dom, body, 3).unwrap();
        assert_eq!(toc.children(toc.root).len(), 2);
        let first = toc.children(toc.root)[0];
        assert_eq!(toc.node(first).text.as_deref(), Some("One"));
        assert_eq!(toc.node(first).fragment.as_deref(), Some("toc_id_1"));
    }

    #[test]
    fn a_deeper_heading_nests_under_its_parent() {
        let mut dom = Dom::empty();
        let body = dom.new_element("body");
        heading(&mut dom, body, "1", "One");
        heading(&mut dom, body, "2", "One point one");

        let toc = from_headings(&mut dom, body, 3).unwrap();
        let top = toc.children(toc.root);
        assert_eq!(top.len(), 1);
        let kids = toc.children(top[0]);
        assert_eq!(kids.len(), 1);
        assert_eq!(toc.node(kids[0]).text.as_deref(), Some("One point one"));
    }

    #[test]
    fn a_skipped_level_climbs_to_the_nearest_open_ancestor() {
        let mut dom = Dom::empty();
        let body = dom.new_element("body");
        heading(&mut dom, body, "1", "One");
        heading(&mut dom, body, "3", "Deeply nested, no h2 preceded it");

        let toc = from_headings(&mut dom, body, 3).unwrap();
        let top = toc.children(toc.root);
        assert_eq!(top.len(), 1);
        let kids = toc.children(top[0]);
        assert_eq!(kids.len(), 1, "attaches under the h1, not the root");
    }

    #[test]
    fn a_level_beyond_num_levels_is_skipped() {
        let mut dom = Dom::empty();
        let body = dom.new_element("body");
        heading(&mut dom, body, "1", "One");
        heading(&mut dom, body, "4", "Beyond num_levels=3");

        let toc = from_headings(&mut dom, body, 3).unwrap();
        assert_eq!(
            toc.children(toc.root).len(),
            1,
            "the h4 never became an entry"
        );
    }

    #[test]
    fn an_existing_id_is_reused_instead_of_generating_one() {
        let mut dom = Dom::empty();
        let body = dom.new_element("body");
        let h = heading(&mut dom, body, "1", "One");
        dom.node_mut(h)
            .attrs
            .insert("id".to_string(), "already-set".to_string());
        heading(&mut dom, body, "1", "Two");

        let toc = from_headings(&mut dom, body, 3).unwrap();
        let first = toc.children(toc.root)[0];
        assert_eq!(toc.node(first).fragment.as_deref(), Some("already-set"));
    }
}

#[cfg(test)]
mod to_ncx_toc_tests {
    use super::*;

    #[test]
    fn headings_become_a_metadata_toc_with_fragment_hrefs() {
        let mut dom = Dom::empty();
        let body = dom.new_element("body");
        let h1 = {
            let h = dom.new_element("h1");
            let t = dom.new_text("Chapter One");
            dom.append_child(h, t);
            dom.append_child(body, h);
            h
        };
        dom.node_mut(h1)
            .attrs
            .insert("data-heading-level".to_string(), "1".to_string());
        let h2 = {
            let h = dom.new_element("h2");
            let t = dom.new_text("Section One");
            dom.append_child(h, t);
            dom.append_child(body, h);
            h
        };
        dom.node_mut(h2)
            .attrs
            .insert("data-heading-level".to_string(), "2".to_string());

        let toc = from_headings(&mut dom, body, 3).unwrap();
        let ncx = toc.to_ncx_toc();

        assert_eq!(ncx.nodes.len(), 1);
        assert_eq!(ncx.nodes[0].title, "Chapter One");
        assert_eq!(ncx.nodes[0].src, "index.html#toc_id_1");
        assert_eq!(ncx.nodes[0].children.len(), 1);
        assert_eq!(ncx.nodes[0].children[0].title, "Section One");
        assert_eq!(ncx.nodes[0].children[0].src, "index.html#toc_id_2");
    }

    #[test]
    fn an_empty_toc_converts_to_an_empty_ncx_toc() {
        let toc = Toc::new();
        let ncx = toc.to_ncx_toc();
        assert!(ncx.nodes.is_empty());
    }
}

#[cfg(test)]
mod create_toc_tests {
    use super::*;

    #[test]
    fn create_toc_strips_data_heading_level_regardless_of_which_path_won() {
        let mut dom = Dom::empty();
        let body = dom.new_element("body");
        let h1 = {
            let h = dom.new_element("p");
            dom.node_mut(h)
                .attrs
                .insert("data-heading-level".to_string(), "1".to_string());
            let t = dom.new_text("One");
            dom.append_child(h, t);
            dom.append_child(body, h);
            h
        };
        let h2 = {
            let h = dom.new_element("p");
            dom.node_mut(h)
                .attrs
                .insert("data-heading-level".to_string(), "1".to_string());
            let t = dom.new_text("Two");
            dom.append_child(h, t);
            dom.append_child(body, h);
            h
        };

        // No `w:document` source to search -- `from_toc` finds nothing,
        // so `from_headings` wins.
        let empty_doc = roxmltree::Document::parse("<w:document xmlns:w=\"x\"/>").unwrap();
        let mut styles = Styles::new(super::super::tables::Tables::default());
        let theme = Theme::new();
        let mut fonts = Fonts::new();
        let object_map = IndexMap::new();
        let ns = DocxNamespace::default();

        let toc = create_toc(
            empty_doc.root_element(),
            &mut dom,
            body,
            &HashMap::new(),
            &mut styles,
            &theme,
            &mut fonts,
            &object_map,
            &ns,
        );
        assert!(toc.is_some());
        assert!(!dom.node(h1).attrs.contains_key("data-heading-level"));
        assert!(!dom.node(h2).attrs.contains_key("data-heading-level"));
    }
}

#[cfg(test)]
mod from_toc_tests {
    use super::*;
    use crate::docx::container::Relationships;
    use crate::docx::tables::Tables;
    use crate::docx::to_html::{convert_p, ConvertState};
    use crate::docx::{Footnotes, Settings};
    use roxmltree::Document;

    const DOC_OPEN: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:xml="http://www.w3.org/XML/1998/namespace" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

    fn parse_root(body: &str) -> (Document<'static>, DocxNamespace) {
        let xml: &'static str =
            Box::leak(format!("<w:document {DOC_OPEN}>{body}</w:document>").into_boxed_str());
        (
            Document::parse(xml).expect("valid XML"),
            DocxNamespace::default(),
        )
    }

    #[test]
    fn a_word_toc_field_becomes_a_flat_toc() {
        // A minimal `{ TOC \o "1-3" }` field: fldChar[begin] ->
        // instrText("TOC ...") -> fldChar[separate, implicit via a
        // second begin/end nesting is unnecessary here] -> hyperlink ->
        // fldChar[end].
        let (doc, ns) = parse_root(
            r#"<w:body>
                 <w:p>
                   <w:r><w:fldChar w:fldCharType="begin"/></w:r>
                   <w:r><w:instrText>TOC \o "1-3" \h \z \u</w:instrText></w:r>
                 </w:p>
                 <w:p>
                   <w:hyperlink w:anchor="chap1"><w:r><w:t>Chapter One</w:t></w:r></w:hyperlink>
                 </w:p>
                 <w:p>
                   <w:r><w:fldChar w:fldCharType="end"/></w:r>
                 </w:p>
               </w:body>"#,
        );
        let document = doc.root_element();
        let mut dom = Dom::empty();
        let mut state = ConvertState::new();
        let mut styles = Styles::new(Tables::default());
        let mut footnotes = Footnotes::new();
        let settings = Settings::new();
        let theme = Theme::new();
        let mut fonts = Fonts::new();

        state
            .anchor_map
            .insert("chap1".to_string(), "id_chap1".to_string());

        let hyperlink_p = ns
            .descendants(document, &["w:p"])
            .into_iter()
            .nth(1)
            .unwrap();
        let mut images = crate::docx::images::Images::new();
        let mut docx = crate::docx::to_html::empty_test_docx();
        let dest_dir = tempfile::tempdir().unwrap();
        convert_p(
            &mut dom,
            &mut state,
            hyperlink_p,
            &mut styles,
            &mut footnotes,
            &settings,
            &theme,
            &mut fonts,
            None,
            "test-uuid",
            &mut images,
            &mut docx,
            dest_dir.path(),
            &crate::docx::styles::PageProperties::default(),
            &Relationships::default(),
            &crate::docx::to_html::AlternateContent::default(),
            &ns,
        );

        let resolved = crate::docx::to_html::resolve_links(
            &mut dom,
            &state,
            &crate::docx::images::Images::new(),
            &[],
            &ns,
        );

        let toc = from_toc(
            document,
            &resolved,
            &mut dom,
            &mut styles,
            &theme,
            &mut fonts,
            &state.object_map,
            &ns,
        )
        .expect("a TOC field with one entry was found");

        let top = toc.children(toc.root);
        assert_eq!(top.len(), 1);
        assert_eq!(toc.node(top[0]).text.as_deref(), Some("Chapter One"));
        assert_eq!(toc.node(top[0]).fragment.as_deref(), Some("id_chap1"));
    }

    #[test]
    fn no_toc_field_yields_none() {
        let (doc, ns) = parse_root(
            r#"<w:body><w:p><w:hyperlink w:anchor="chap1"><w:r><w:t>x</w:t></w:r></w:hyperlink></w:p></w:body>"#,
        );
        let document = doc.root_element();
        let mut dom = Dom::empty();
        let mut styles = Styles::new(Tables::default());
        let theme = Theme::new();
        let mut fonts = Fonts::new();
        let object_map = IndexMap::new();

        let toc = from_toc(
            document,
            &HashMap::new(),
            &mut dom,
            &mut styles,
            &theme,
            &mut fonts,
            &object_map,
            &ns,
        );
        assert!(toc.is_none());
    }
}
