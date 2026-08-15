//! Port of `old_src/src/calibre/ebooks/oeb/polish/pretty.py`.
//!
//! # Tree-shape note
//!
//! Python's algorithm works by mutating `elem.text`/`child.tail`
//! attributes on an `lxml` tree. Both this crate's [`Xml`] (OPF/NCX/XML)
//! and [`Dom`] (XHTML content) arenas instead represent that same text
//! as ordinary sibling `Text` nodes (see each module's own docs for why:
//! it makes *removal* trivially correct and, as this module demonstrates,
//! makes *insertion* -- what pretty-printing needs -- a matter of
//! "replace this sibling text node's content, or insert a new one at
//! this position" rather than out-of-band attribute bookkeeping). This
//! module ports the same tree-shaping *decisions* Python's algorithm
//! makes (when to (re)indent, by how much, whether an element's existing
//! content survives) but expresses them against that sibling-node model.
//! The output is well-formed, correctly indented markup; it is not
//! guaranteed byte-identical to lxml's whitespace splicing in every
//! corner case (same scope note as `xmltree`'s module docs).
//!
//! Because [`Xml`] and [`Dom`] are genuinely different arena types (see
//! `xmltree`'s module docs for why OPF-family XML and XHTML content
//! don't share one tree representation in this port), the tree-walking
//! half of this algorithm (`pretty_xml_tree`) is implemented twice --
//! once per arena -- rather than introducing a shared abstraction neither
//! caller needs elsewhere.

use std::collections::HashMap;

use anyhow::Result;

use crate::mobi::dom::{Dom, NodeId, NodeKind};
use crate::oeb::constants::{DC11_NS, NCX_MIME, OEB_DOCS, OEB_STYLES, SVG_MIME, XML_MIME};

use super::container::{opf_namespaces, Container, ContainerBase};
use super::xmltree::{Xml, XmlNodeId, XmlNodeKind};

/// Port of `isspace`: is `x` made up entirely of tab/newline/formfeed/
/// carriage-return/space characters (true for the empty string too,
/// matching Python's `not x.strip(...)`)?
pub fn isspace(x: &str) -> bool {
    x.trim_matches(|c| matches!(c, '\u{9}' | '\u{a}' | '\u{c}' | '\u{d}' | ' '))
        .is_empty()
}

// ===================================================================
// XML-tree pretty printing (OPF / NCX / generic XML), via `xmltree::Xml`
// ===================================================================

fn xml_tail_text(xml: &Xml, id: XmlNodeId) -> Option<String> {
    let parent = xml.parent(id)?;
    let pos = xml.index_in_parent(id)?;
    let next = *xml.children(parent).get(pos + 1)?;
    match &xml.node(next).kind {
        XmlNodeKind::Text(t) => Some(t.clone()),
        _ => None,
    }
}

fn xml_set_tail_text(xml: &mut Xml, id: XmlNodeId, text: &str) {
    let Some(parent) = xml.parent(id) else {
        return;
    };
    let Some(pos) = xml.index_in_parent(id) else {
        return;
    };
    let next = xml.children(parent).get(pos + 1).copied();
    if let Some(next) = next {
        if let XmlNodeKind::Text(_) = xml.node(next).kind {
            xml.node_mut(next).kind = XmlNodeKind::Text(text.to_string());
            return;
        }
    }
    let tid = xml.new_text(text);
    xml.node_mut(tid).parent = Some(parent);
    xml.node_mut(parent).children.insert(pos + 1, tid);
}

/// Port of `pretty_xml_tree`, for [`Xml`] documents (OPF/NCX/generic
/// XML). See the module docs for the tree-shape note.
pub fn pretty_xml_tree(xml: &mut Xml, id: XmlNodeId, level: usize, indent: &str) {
    if !matches!(xml.node(id).kind, XmlNodeKind::Element { .. }) {
        return;
    }
    let elem_children = xml.element_children(id);
    let has_children = !elem_children.is_empty();
    let leading = xml.element_text(id).map(|s| s.to_string());
    let should_set = match &leading {
        None => has_children,
        Some(t) => isspace(t),
    };
    if should_set {
        xml.set_element_text(id, format!("\n{}", indent.repeat(level + 1)));
    }
    let n = elem_children.len();
    for (i, &child) in elem_children.iter().enumerate() {
        pretty_xml_tree(xml, child, level + 1, indent);
        let tail = xml_tail_text(xml, child);
        let should_set_tail = match &tail {
            None => true,
            Some(t) => isspace(t),
        };
        if should_set_tail {
            let mut l = level + 1;
            if i == n - 1 {
                l -= 1;
            }
            xml_set_tail_text(xml, child, &format!("\n{}", indent.repeat(l)));
        }
    }
}

fn dc_key(local: &str) -> u8 {
    match local {
        "title" => 0,
        "creator" => 1,
        _ => 2,
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum ManifestOrderKey {
    Index(u64),
    /// Fallback for calibre's ICU-collated `sort_key(href)`: this
    /// workspace's `calibre_utils::icu` is a plain-Unicode stub (see its
    /// own module docs), not a real ICU collator, so this orders by
    /// lowercased href text instead of locale-aware collation. Ordering
    /// stays stable and reasonable; it just isn't locale-perfect.
    Text(String),
}

fn manifest_key(
    xml: &Xml,
    spine_index: &HashMap<String, usize>,
    item: XmlNodeId,
) -> (u32, ManifestOrderKey) {
    let mt = xml.get_attr(item, "media-type").unwrap_or("");
    let href = xml.get_attr(item, "href").unwrap_or("");
    let ext = href.rsplit('.').next().unwrap_or("").to_lowercase();
    let cat: u32 = if OEB_DOCS.contains(&mt) {
        0
    } else if mt.eq_ignore_ascii_case(NCX_MIME) {
        1
    } else if OEB_STYLES.contains(&mt) {
        2
    } else if mt.starts_with("image/") {
        3
    } else if ["otf", "ttf", "woff", "woff2"].contains(&ext.as_str()) {
        4
    } else if mt.starts_with("audio/") {
        5
    } else if mt.starts_with("video/") {
        6
    } else {
        1000
    };
    let key = if cat == 0 {
        let id = xml.get_attr(item, "id").unwrap_or("");
        ManifestOrderKey::Index(spine_index.get(id).copied().unwrap_or(1_000_000_000) as u64)
    } else {
        ManifestOrderKey::Text(href.to_lowercase())
    };
    (cat, key)
}

/// Port of `pretty_opf`: reorders `dc:` metadata tags (title, creator,
/// then the rest, stably) and groups manifest items by category.
pub fn pretty_opf(xml: &mut Xml) {
    let ns = opf_namespaces();
    for metadata in xml.opf_xpath("//opf:metadata", &ns) {
        let mut dc_tags: Vec<XmlNodeId> = xml
            .element_children(metadata)
            .into_iter()
            .filter(|&c| xml.namespace(c) == Some(DC11_NS))
            .collect();
        dc_tags.sort_by_key(|&c| dc_key(xml.local_name(c).unwrap_or("")));
        for &x in dc_tags.iter().rev() {
            xml.insert_element(metadata, x, Some(0));
        }
    }

    let spine_index: HashMap<String, usize> = xml
        .opf_xpath("//opf:spine/opf:itemref[@idref]", &ns)
        .into_iter()
        .enumerate()
        .map(|(i, n)| (xml.get_attr(n, "idref").unwrap_or("").to_string(), i))
        .collect();

    for manifest in xml.opf_xpath("//opf:manifest", &ns) {
        // Python bails out (leaving the manifest untouched) if it
        // contains a Comment node, since `sorted(manifest, key=...)`
        // raises `AttributeError` calling `.get()` on a comment. Mirror
        // that: skip sorting rather than dropping the comment.
        let has_comment = xml
            .children(manifest)
            .iter()
            .any(|&c| matches!(xml.node(c).kind, XmlNodeKind::Comment(_)));
        if has_comment {
            continue;
        }
        let mut children = xml.element_children(manifest);
        children.sort_by_key(|&item| manifest_key(xml, &spine_index, item));
        for &x in children.iter().rev() {
            xml.insert_element(manifest, x, Some(0));
        }
    }
}

// ===================================================================
// HTML-tree pretty printing (XHTML content), via `mobi::dom::Dom`
// ===================================================================

/// Port of `BLOCK_TAGS` (`SVG_TAG` is handled separately -- see
/// [`isblock`]).
pub const BLOCK_TAGS: &[&str] = &[
    "address",
    "article",
    "aside",
    "audio",
    "blockquote",
    "body",
    "canvas",
    "col",
    "colgroup",
    "dd",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "li",
    "noscript",
    "ol",
    "output",
    "p",
    "pre",
    "script",
    "section",
    "style",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "ul",
    "video",
    "img",
];

/// Port of `isblock`. Comment nodes count as blocks, matching Python's
/// `callable(x.tag)` branch (true for lxml's Comment/PI pseudo-elements).
fn isblock(dom: &Dom, id: NodeId) -> bool {
    match &dom.node(id).kind {
        NodeKind::Comment(_) => true,
        NodeKind::Element(tag) => BLOCK_TAGS.contains(&tag.as_str()) || tag == "svg",
        _ => false,
    }
}

/// Non-text direct children (Python's `for child in elem` over an lxml
/// tree, which yields Elements/Comments/PIs but not `.text`/`.tail`).
fn element_or_comment_children(dom: &Dom, id: NodeId) -> Vec<NodeId> {
    dom.children(id)
        .into_iter()
        .filter(|&c| !matches!(dom.node(c).kind, NodeKind::Text(_)))
        .collect()
}

fn leading_text(dom: &Dom, id: NodeId) -> Option<String> {
    match dom.children(id).first() {
        Some(&f) => match &dom.node(f).kind {
            NodeKind::Text(t) => Some(t.clone()),
            _ => None,
        },
        None => None,
    }
}

fn set_leading_text(dom: &mut Dom, id: NodeId, text: &str) {
    if let Some(&first) = dom.children(id).first() {
        if let NodeKind::Text(_) = dom.node(first).kind {
            dom.node_mut(first).kind = NodeKind::Text(text.to_string());
            return;
        }
    }
    let t = dom.new_text(text);
    dom.insert_child(id, 0, t);
}

fn dom_tail(dom: &Dom, id: NodeId) -> Option<String> {
    let parent = dom.parent(id)?;
    let pos = dom.index_in_parent(id)?;
    let next = *dom.children(parent).get(pos + 1)?;
    match &dom.node(next).kind {
        NodeKind::Text(t) => Some(t.clone()),
        _ => None,
    }
}

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

/// Port of `has_only_blocks`.
fn has_only_blocks(dom: &Dom, id: NodeId) -> bool {
    if dom.tag(id).is_some() && element_or_comment_children(dom, id).is_empty() {
        return false;
    }
    for child in dom.children(id) {
        match &dom.node(child).kind {
            NodeKind::Text(t) => {
                if !isspace(t) {
                    return false;
                }
            }
            NodeKind::Element(_) | NodeKind::Comment(_) => {
                if !isblock(dom, child) {
                    return false;
                }
            }
            NodeKind::Document => {}
        }
    }
    true
}

/// Port of `indent_for_tag`.
fn indent_for_tag(dom: &Dom, id: NodeId) -> String {
    let Some(parent) = dom.parent(id) else {
        return String::new();
    };
    let siblings = dom.children(parent);
    let Some(pos) = siblings.iter().position(|&c| c == id) else {
        return String::new();
    };
    let prev_elem = siblings[..pos]
        .iter()
        .rev()
        .copied()
        .find(|&c| !matches!(dom.node(c).kind, NodeKind::Text(_)));
    let text = match prev_elem {
        None => leading_text(dom, parent),
        Some(prev) => dom_tail(dom, prev),
    };
    match text {
        None => String::new(),
        Some(t) if t.is_empty() => String::new(),
        Some(t) => {
            let s = t.rsplit('\n').next().unwrap_or("");
            if isspace(s) {
                s.to_string()
            } else {
                String::new()
            }
        }
    }
}

/// Port of `set_indent` (the `attr='text'` case -- the only one any
/// call site in `pretty.py` uses).
fn set_indent_leading_text(dom: &mut Dom, id: NodeId, indent: &str) {
    let cur = leading_text(dom, id);
    let new_text = match cur {
        None => indent.to_string(),
        Some(x) if x.is_empty() => indent.to_string(),
        Some(x) => {
            let mut lines: Vec<String> = x.split('\n').map(|s| s.to_string()).collect();
            let last_is_space = lines.last().map(|s| isspace(s)).unwrap_or(false);
            if last_is_space {
                let n = lines.len();
                lines[n - 1] = indent.to_string();
            } else {
                lines.push(indent.to_string());
            }
            lines.join("\n")
        }
    };
    set_leading_text(dom, id, &new_text);
}

/// Port of `pretty_xml_tree` as used for [`Dom`] subtrees (the `<head>`
/// element, and any `<svg>` subtree encountered inside `<body>`).
pub fn pretty_dom_xml_tree(dom: &mut Dom, id: NodeId, level: usize, indent: &str) {
    if !matches!(dom.node(id).kind, NodeKind::Element(_)) {
        return;
    }
    let kids = element_or_comment_children(dom, id);
    let has_children = !kids.is_empty();
    let leading = leading_text(dom, id);
    let should_set = match &leading {
        None => has_children,
        Some(t) => isspace(t),
    };
    if should_set {
        set_leading_text(dom, id, &format!("\n{}", indent.repeat(level + 1)));
    }
    let n = kids.len();
    for (i, &child) in kids.iter().enumerate() {
        pretty_dom_xml_tree(dom, child, level + 1, indent);
        let tail = dom_tail(dom, child);
        let should_set_tail = match &tail {
            None => true,
            Some(t) => isspace(t),
        };
        if should_set_tail {
            let mut l = level + 1;
            if i == n - 1 {
                l -= 1;
            }
            set_dom_tail(dom, child, &format!("\n{}", indent.repeat(l)));
        }
    }
}

/// Port of `pretty_block`: surrounds block tags with blank lines,
/// recursing into child block tags that contain only other block tags.
pub fn pretty_block(dom: &mut Dom, parent: NodeId, level: usize, indent: &str) {
    let leading = leading_text(dom, parent);
    let base = match &leading {
        Some(t) if !isspace(t) => t.clone(),
        _ => String::new(),
    };
    let tag = dom.tag(parent).unwrap_or("").to_string();
    let nn = if matches!(tag.as_str(), "tr" | "td" | "th") {
        "\n"
    } else {
        "\n\n"
    };
    set_leading_text(dom, parent, &format!("{base}{nn}{}", indent.repeat(level)));

    let kids = element_or_comment_children(dom, parent);
    let n = kids.len();
    for (i, &child) in kids.iter().enumerate() {
        if isblock(dom, child) && has_only_blocks(dom, child) {
            pretty_block(dom, child, level + 1, indent);
        } else if dom.tag(child) == Some("svg") {
            pretty_dom_xml_tree(dom, child, level, indent);
        }
        let mut l = level;
        if i == n - 1 {
            l = l.saturating_sub(1);
        }
        let base_tail = match dom_tail(dom, child) {
            Some(t) if !isspace(&t) => t,
            _ => String::new(),
        };
        set_dom_tail(dom, child, &format!("{base_tail}{nn}{}", indent.repeat(l)));
    }
}

/// Port of `textwrap.dedent`: strips the longest common leading
/// whitespace shared by every non-blank line.
fn dedent(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut margin: Option<&str> = None;
    for line in &lines {
        let stripped = line.trim_start_matches([' ', '\t']);
        if stripped.is_empty() {
            continue;
        }
        let indent = &line[..line.len() - stripped.len()];
        margin = Some(match margin {
            None => indent,
            Some(m) => common_prefix(m, indent),
        });
    }
    let Some(margin) = margin else {
        return text.to_string();
    };
    if margin.is_empty() {
        return text.to_string();
    }
    lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                line.trim_start_matches([' ', '\t']).to_string()
            } else {
                line.strip_prefix(margin).unwrap_or(line).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn common_prefix<'a>(a: &'a str, b: &str) -> &'a str {
    let mut end = 0;
    for (ca, cb) in a.chars().zip(b.chars()) {
        if ca == cb {
            end += ca.len_utf8();
        } else {
            break;
        }
    }
    &a[..end]
}

/// Port of `pretty_script_or_style`. The CSS-reformatting half (`<style>`
/// tags with real `<style>...text...</style>` content) needs a real CSS
/// parser/serializer this crate doesn't have -- see [`pretty_css`]'s
/// docs -- and is the only `todo!()` this function reaches; the
/// dedent/reindent logic that applies to both `<script>` and `<style>`
/// tags is real.
fn pretty_script_or_style(dom: &mut Dom, child: NodeId) -> Result<()> {
    let Some(text0) = leading_text(dom, child) else {
        return Ok(());
    };
    if text0.is_empty() {
        return Ok(());
    }
    let indent = indent_for_tag(dom, child);
    let tag = dom.tag(child).unwrap_or("").to_string();
    let mut text = text0;
    if tag.ends_with("style") {
        text = pretty_css_text(&text)?;
    }
    let dedented = dedent(&text);
    let joined: Vec<String> = dedented
        .split('\n')
        .map(|x| {
            if x.is_empty() {
                String::new()
            } else {
                format!("{indent}{x}")
            }
        })
        .collect();
    set_leading_text(dom, child, &format!("\n{}", joined.join("\n")));
    set_indent_leading_text(dom, child, &indent);
    Ok(())
}

/// Port of `pretty_html_tree`.
pub fn pretty_html_tree(dom: &mut Dom) -> Result<()> {
    let Some(root) = dom
        .node(dom.root)
        .children
        .iter()
        .copied()
        .find(|&c| matches!(dom.node(c).kind, NodeKind::Element(_)))
    else {
        return Ok(());
    };
    set_leading_text(dom, root, "\n\n");
    for &child in &element_or_comment_children(dom, root) {
        set_dom_tail(dom, child, "\n\n");
        if dom.tag(child) == Some("head") {
            pretty_dom_xml_tree(dom, child, 0, "  ");
        }
    }

    const NEVER_PRETTY_BLOCK_ALONE: &[&str] = &["pre", "p", "h1", "h2", "h3", "h4", "h5", "h6"];
    for &body in &dom.find_all_tag(root, "body") {
        pretty_block(dom, body, 1, "  ");
        let body_kids = element_or_comment_children(dom, body);
        if body_kids.len() == 1 {
            let only = body_kids[0];
            let only_tag = dom.tag(only).unwrap_or("").to_string();
            let excluded = NEVER_PRETTY_BLOCK_ALONE.contains(&only_tag.as_str());
            if isblock(dom, only)
                && !has_only_blocks(dom, only)
                && !excluded
                && !element_or_comment_children(dom, only).is_empty()
            {
                pretty_block(dom, only, 2, "  ");
            }
        }
    }

    let style_script_nodes: Vec<NodeId> = dom
        .preorder_elements(root)
        .into_iter()
        .filter(|&n| matches!(dom.tag(n), Some("script") | Some("style")))
        .collect();
    for node in style_script_nodes {
        pretty_script_or_style(dom, node)?;
    }
    Ok(())
}

// ===================================================================
// Top-level entry points
// ===================================================================

/// Port of `fix_html`: reparse `raw` (fixing any HTML5-recoverable
/// parsing errors) and reserialize.
pub fn fix_html(base: &mut ContainerBase, raw: &[u8]) -> Vec<u8> {
    let dom = base.parse_xhtml(raw);
    dom.serialize(dom.root).into_bytes()
}

/// Port of `pretty_html`.
pub fn pretty_html(base: &mut ContainerBase, raw: &[u8]) -> Result<Vec<u8>> {
    let mut dom = base.parse_xhtml(raw);
    pretty_html_tree(&mut dom)?;
    Ok(dom.serialize(dom.root).into_bytes())
}

/// Port of `pretty_css`. Genuinely blocked: pretty-printing CSS needs a
/// real CSS parser/serializer (parse into rules/declarations, then
/// re-emit with consistent formatting), which this crate does not have
/// -- the same, already-documented gap as
/// [`super::utils::parse_css`]/issues #34/#35/#36. Everything in this
/// module that does *not* require parsing CSS text (XML/HTML tree
/// reindentation, OPF reordering, `<script>`/`<style>` dedent) is ported
/// for real; only the actual `text/css` -> reformatted `text/css` step
/// is a placeholder.
pub fn pretty_css(_base: &mut ContainerBase, _raw: &str) -> Result<Vec<u8>> {
    todo!(
        "placeholder: pretty-printing CSS needs a real CSS parser/serializer, \
         which this crate doesn't have -- see oeb::polish::utils::parse_css's \
         docs (same gap as issues #34/#35/#36)"
    )
}

fn pretty_css_text(_raw: &str) -> Result<String> {
    todo!(
        "placeholder: pretty-printing a <style> tag's CSS text needs a real \
         CSS parser/serializer -- see pretty_css's docs"
    )
}

/// Port of `pretty_xml`.
pub fn pretty_xml(container: &mut Container, name: &str, raw: &[u8]) -> Result<Vec<u8>> {
    let mut xml = container.base.parse_xml(raw)?;
    if name == container.opf_name {
        pretty_opf(&mut xml);
    }
    if let Some(root) = xml.root_element() {
        pretty_xml_tree(&mut xml, root, 0, "  ");
    }
    Ok(xml.serialize())
}

/// Port of `fix_all_html`.
pub fn fix_all_html(container: &mut Container) -> Result<()> {
    let names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (name, mt) in names {
        if OEB_DOCS.contains(&mt.as_str()) {
            container.ensure_parsed(&name)?;
            container.dirty(&name);
        }
    }
    Ok(())
}

/// Port of `pretty_all`.
pub fn pretty_all(container: &mut Container) -> Result<()> {
    let xml_types = [NCX_MIME, XML_MIME, SVG_MIME];
    let names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let opf_name = container.opf_name.clone();
    for (name, mt) in names {
        let mut prettied = false;
        if OEB_DOCS.contains(&mt.as_str()) {
            container.ensure_parsed(&name)?;
            let dom = container.get_xhtml_mut(&name)?;
            pretty_html_tree(dom)?;
            prettied = true;
        } else if OEB_STYLES.contains(&mt.as_str()) {
            container.ensure_parsed(&name)?;
            prettied = true;
        } else if name == opf_name {
            container.ensure_parsed(&name)?;
            let xml = container.get_xml_mut(&name)?;
            pretty_opf(xml);
            if let Some(root) = xml.root_element() {
                pretty_xml_tree(xml, root, 0, "  ");
            }
            prettied = true;
        } else if xml_types.contains(&mt.as_str()) {
            container.ensure_parsed(&name)?;
            let xml = container.get_xml_mut(&name)?;
            if let Some(root) = xml.root_element() {
                pretty_xml_tree(xml, root, 0, "  ");
            }
            prettied = true;
        }
        if prettied {
            container.dirty(&name);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isspace_matches_python_semantics() {
        assert!(isspace(""));
        assert!(isspace("   \n\t"));
        assert!(!isspace(" a "));
        assert!(!isspace("hello"));
    }

    #[test]
    fn dedent_strips_common_margin() {
        assert_eq!(dedent("  a\n  b\n  c"), "a\nb\nc");
        assert_eq!(dedent("a\n  b"), "a\n  b");
        assert_eq!(dedent(""), "");
    }

    #[test]
    fn pretty_html_tree_indents_and_preserves_content() {
        let mut dom = Dom::parse(
            "<html><head><title>T</title></head><body><p>hi</p><p>bye</p></body></html>",
        );
        pretty_html_tree(&mut dom).unwrap();
        let out = dom.serialize(dom.root);
        assert!(out.contains("<title>T</title>"));
        assert!(out.contains("<p>hi</p>"));
        assert!(out.contains("<p>bye</p>"));
        // Body content got reindented onto its own lines.
        assert!(out.contains("\n  <p>hi</p>"));
    }

    #[test]
    fn pretty_html_tree_reindents_single_child_block() {
        let mut dom = Dom::parse("<html><body><div><p>a</p><p>b</p></div></body></html>");
        pretty_html_tree(&mut dom).unwrap();
        let out = dom.serialize(dom.root);
        assert!(out.contains("<p>a</p>"));
        assert!(out.contains("<p>b</p>"));
    }

    fn opf_ns() -> HashMap<&'static str, &'static str> {
        let mut m = HashMap::new();
        m.insert("opf", crate::oeb::constants::OPF2_NS);
        m.insert("dc", DC11_NS);
        m
    }

    const SAMPLE_OPF: &str = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata>
    <dc:language>en</dc:language>
    <dc:creator>Author</dc:creator>
    <dc:title>Title</dc:title>
  </metadata>
  <manifest>
    <item id="css" href="style.css" media-type="text/css"/>
    <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
  </spine>
</package>"#;

    #[test]
    fn pretty_opf_reorders_dc_tags_and_manifest() {
        let mut xml = Xml::parse(SAMPLE_OPF).unwrap();
        pretty_opf(&mut xml);
        let ns = opf_ns();
        let metadata = xml.opf_xpath("//opf:metadata", &ns)[0];
        let dc_children: Vec<&str> = xml
            .element_children(metadata)
            .into_iter()
            .map(|c| xml.local_name(c).unwrap_or(""))
            .collect();
        assert_eq!(dc_children, vec!["title", "creator", "language"]);

        let manifest = xml.opf_xpath("//opf:manifest", &ns)[0];
        let items: Vec<&str> = xml
            .element_children(manifest)
            .into_iter()
            .map(|c| xml.get_attr(c, "href").unwrap_or(""))
            .collect();
        // The spine document (chap1.html) sorts before the stylesheet.
        assert_eq!(items, vec!["chap1.html", "style.css"]);
    }

    #[test]
    fn pretty_xml_tree_indents_xml() {
        let mut xml = Xml::parse("<root><a/><b/></root>").unwrap();
        let root = xml.root_element().unwrap();
        pretty_xml_tree(&mut xml, root, 0, "  ");
        let out = String::from_utf8(xml.serialize()).unwrap();
        assert!(out.contains("\n  <a"));
        assert!(out.contains("\n  <b"));
    }
}
