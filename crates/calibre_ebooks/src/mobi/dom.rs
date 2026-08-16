//! A small, mutable arena-based HTML tree used by the parts of the MOBI
//! reader that Python implements with `lxml.etree`/`lxml.html`
//! (`mobi6.py`'s `upshift_markup`/`create_opf`/`read_embedded_metadata`,
//! `mobi8.py`'s `read_inline_toc`).
//!
//! Real parsing is delegated to `html5ever` (via `markup5ever_rcdom`),
//! matching the precedent in `calibre_conversion::transform::html_roundtrip`.
//! `markup5ever_rcdom::Node` keeps its element name as an immutable
//! `QualName`, which makes lxml-style in-place tag renaming
//! (`tag.tag = 'span'`) awkward to express directly against it. Rather than
//! fight that, the parsed rcdom tree is converted once into this simpler
//! arena (`Vec<Node>` + index-based parent/child links, tag name as a plain
//! `String`), all mutation happens here, and the result is serialized back
//! to a string with a small recursive writer. This is still "real" DOM tree
//! walking + mutation -- html5ever does the actual HTML5 parsing -- just
//! with a representation that is far less friction for a lxml-shaped port.

use html5ever::tendril::TendrilSink;
use html5ever::{parse_document, LocalName};
use indexmap::IndexMap;
use markup5ever_rcdom::{Handle as RcHandle, NodeData as RcNodeData, RcDom};

pub type NodeId = usize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    Document,
    Element(String),
    Text(String),
    Comment(String),
}

#[derive(Debug, Clone)]
pub struct Node {
    pub kind: NodeKind,
    pub attrs: IndexMap<String, String>,
    pub children: Vec<NodeId>,
    pub parent: Option<NodeId>,
}

#[derive(Clone)]
pub struct Dom {
    pub nodes: Vec<Node>,
    pub root: NodeId,
}

impl Dom {
    /// Parses `html` as an HTML5 document (tag-soup tolerant, like
    /// `lxml.html`/`html5-parser`).
    pub fn parse(html: &str) -> Dom {
        let rc_dom = parse_document(RcDom::default(), Default::default())
            .from_utf8()
            .read_from(&mut html.as_bytes())
            .unwrap_or_default();

        let mut nodes = Vec::new();
        let root = convert(&rc_dom.document, &mut nodes, None);
        Dom { nodes, root }
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id]
    }

    pub fn tag(&self, id: NodeId) -> Option<&str> {
        match &self.nodes[id].kind {
            NodeKind::Element(t) => Some(t.as_str()),
            _ => None,
        }
    }

    pub fn set_tag(&mut self, id: NodeId, new_tag: &str) {
        if let NodeKind::Element(_) = &self.nodes[id].kind {
            self.nodes[id].kind = NodeKind::Element(new_tag.to_string());
        }
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id].parent
    }

    pub fn children(&self, id: NodeId) -> Vec<NodeId> {
        self.nodes[id].children.clone()
    }

    /// The literal next sibling (of any kind -- text, comment, or
    /// element). This is what corresponds to lxml's `tag.tail`: if it's a
    /// `Text` node, that text is the "tail" of `id`.
    pub fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.nodes[id].parent?;
        let siblings = &self.nodes[parent].children;
        let pos = siblings.iter().position(|&c| c == id)?;
        siblings.get(pos + 1).copied()
    }

    /// The next sibling that is an `Element`, skipping over any
    /// interleaving text/comment nodes. Matches lxml's `tag.getnext()`.
    pub fn next_element_sibling(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.nodes[id].parent?;
        let siblings = &self.nodes[parent].children;
        let pos = siblings.iter().position(|&c| c == id)?;
        siblings[pos + 1..]
            .iter()
            .copied()
            .find(|&c| matches!(self.nodes[c].kind, NodeKind::Element(_)))
    }

    /// The literal previous sibling (of any kind -- text, comment, or
    /// element). Matches lxml's `el.itersiblings(preceding=True)` walked
    /// one step, or (for the element-only case) `tag.getprevious()`.
    /// Added for `calibre::ebooks::readability`'s `sanitize`, which walks
    /// preceding siblings to sum up nearby content length; every other
    /// caller of this module only ever needed `next_sibling`.
    pub fn prev_sibling(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.nodes[id].parent?;
        let siblings = &self.nodes[parent].children;
        let pos = siblings.iter().position(|&c| c == id)?;
        if pos == 0 {
            None
        } else {
            siblings.get(pos - 1).copied()
        }
    }

    pub fn index_in_parent(&self, id: NodeId) -> Option<usize> {
        let parent = self.nodes[id].parent?;
        self.nodes[parent].children.iter().position(|&c| c == id)
    }

    /// Creates a new, parentless element node and returns its id.
    pub fn new_element(&mut self, tag: &str) -> NodeId {
        self.nodes.push(Node {
            kind: NodeKind::Element(tag.to_string()),
            attrs: IndexMap::new(),
            children: Vec::new(),
            parent: None,
        });
        self.nodes.len() - 1
    }

    pub fn new_text(&mut self, text: &str) -> NodeId {
        self.nodes.push(Node {
            kind: NodeKind::Text(text.to_string()),
            attrs: IndexMap::new(),
            children: Vec::new(),
            parent: None,
        });
        self.nodes.len() - 1
    }

    /// Detaches `id` from its parent's child list (the node itself stays
    /// alive in the arena, orphaned, so other `NodeId`s referencing it or
    /// its descendants remain valid).
    pub fn detach(&mut self, id: NodeId) {
        if let Some(parent) = self.nodes[id].parent.take() {
            self.nodes[parent].children.retain(|&c| c != id);
        }
    }

    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        self.detach(child);
        self.nodes[child].parent = Some(parent);
        self.nodes[parent].children.push(child);
    }

    pub fn insert_child(&mut self, parent: NodeId, index: usize, child: NodeId) {
        self.detach(child);
        self.nodes[child].parent = Some(parent);
        let idx = index.min(self.nodes[parent].children.len());
        self.nodes[parent].children.insert(idx, child);
    }

    /// Removes `id` from the tree, reparenting its children to its former
    /// parent at the position `id` occupied (mirrors lxml's
    /// `parent.remove(el)` semantics when callers want the subtree's
    /// content to survive -- most `upshift_markup` call sites want the
    /// element gone but its children promoted).
    pub fn remove_promoting_children(&mut self, id: NodeId) {
        let Some(parent) = self.nodes[id].parent else {
            return;
        };
        let Some(pos) = self.index_in_parent(id) else {
            return;
        };
        let kids = std::mem::take(&mut self.nodes[id].children);
        self.nodes[id].parent = None;
        self.nodes[parent].children.remove(pos);
        for (i, kid) in kids.into_iter().enumerate() {
            self.nodes[kid].parent = Some(parent);
            self.nodes[parent].children.insert(pos + i, kid);
        }
    }

    /// Pre-order traversal of every `Element` node starting at (and
    /// including) `id`.
    pub fn preorder_elements(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            if matches!(self.nodes[n].kind, NodeKind::Element(_)) {
                out.push(n);
            }
            for &c in self.nodes[n].children.iter().rev() {
                stack.push(c);
            }
        }
        // stack-based traversal above visits in a DFS order but pushes
        // children in reverse so pops come out in document order; `id`
        // itself is visited first since it's the only start element.
        out
    }

    /// All `Element` descendants of `id` (excluding `id` itself), in
    /// document order.
    pub fn find_all_tag(&self, id: NodeId, tag: &str) -> Vec<NodeId> {
        self.preorder_elements(id)
            .into_iter()
            .filter(|&n| n != id && self.tag(n) == Some(tag))
            .collect()
    }

    /// Every element in the whole document matching `tag` (`root.xpath('//tag')`).
    pub fn find_all_tag_global(&self, tag: &str) -> Vec<NodeId> {
        self.preorder_elements(self.root)
            .into_iter()
            .filter(|&n| self.tag(n) == Some(tag))
            .collect()
    }

    pub fn find_first_tag_global(&self, tag: &str) -> Option<NodeId> {
        self.find_all_tag_global(tag).into_iter().next()
    }

    pub fn find_by_id(&self, id_attr: &str) -> Option<NodeId> {
        self.preorder_elements(self.root)
            .into_iter()
            .find(|&n| self.nodes[n].attrs.get("id").map(|s| s.as_str()) == Some(id_attr))
    }

    /// Concatenated text of every `Text` descendant of `id`, matching
    /// `xml2text`/`x.xpath('descendant::text()')` usage.
    pub fn text_content(&self, id: NodeId) -> String {
        let mut out = String::new();
        let mut stack = vec![id];
        let mut order = Vec::new();
        while let Some(n) = stack.pop() {
            order.push(n);
            for &c in self.nodes[n].children.iter().rev() {
                stack.push(c);
            }
        }
        order.reverse();
        for n in order {
            if let NodeKind::Text(t) = &self.nodes[n].kind {
                out.push_str(t);
            }
        }
        out
    }

    /// Renders just `id`'s opening tag (`<tag attr="val" ...>`), with no
    /// children or closing tag. Used by
    /// [`crate::mobi::writer8::skeleton`] to measure exactly how many
    /// bytes an element's open tag occupies in the serialized skeleton,
    /// consistently with what [`Dom::serialize`] would produce for the
    /// same element (both share the same attribute-writing logic).
    /// Void elements still render as `<tag ... />` here even though
    /// `serialize` only does that when they have no children -- callers
    /// measuring open-tag length for a *non*-void element (the only kind
    /// `writer8::skeleton` ever calls this on, since only elements able
    /// to carry an `aid` are measured) are unaffected either way.
    pub fn serialize_open_tag(&self, id: NodeId) -> String {
        let node = &self.nodes[id];
        let mut out = String::new();
        if let NodeKind::Element(tag) = &node.kind {
            out.push('<');
            out.push_str(tag);
            for (k, v) in &node.attrs {
                out.push(' ');
                out.push_str(k);
                out.push_str("=\"");
                out.push_str(&html_escape::encode_double_quoted_attribute(v));
                out.push('"');
            }
            out.push('>');
        }
        out
    }

    /// Serializes the subtree rooted at `id` as HTML.
    pub fn serialize(&self, id: NodeId) -> String {
        let mut out = String::new();
        self.serialize_into(id, &mut out);
        out
    }

    fn serialize_into(&self, id: NodeId, out: &mut String) {
        let node = &self.nodes[id];
        match &node.kind {
            NodeKind::Document => {
                for &c in &node.children {
                    self.serialize_into(c, out);
                }
            }
            NodeKind::Text(t) => out.push_str(&html_escape::encode_text(t)),
            NodeKind::Comment(t) => {
                out.push_str("<!--");
                out.push_str(t);
                out.push_str("-->");
            }
            NodeKind::Element(tag) => {
                out.push('<');
                out.push_str(tag);
                for (k, v) in &node.attrs {
                    out.push(' ');
                    out.push_str(k);
                    out.push_str("=\"");
                    out.push_str(&html_escape::encode_double_quoted_attribute(v));
                    out.push('"');
                }
                if node.children.is_empty() && is_void_element(tag) {
                    out.push_str(" />");
                    return;
                }
                out.push('>');
                for &c in &node.children {
                    self.serialize_into(c, out);
                }
                out.push_str("</");
                out.push_str(tag);
                out.push('>');
            }
        }
    }
}

fn is_void_element(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn convert(handle: &RcHandle, nodes: &mut Vec<Node>, parent: Option<NodeId>) -> NodeId {
    let kind = match &handle.data {
        RcNodeData::Document => NodeKind::Document,
        RcNodeData::Doctype { .. } => NodeKind::Comment(String::new()),
        RcNodeData::Text { contents } => NodeKind::Text(contents.borrow().to_string()),
        RcNodeData::Comment { contents } => NodeKind::Comment(contents.to_string()),
        RcNodeData::Element { name, attrs, .. } => {
            let tag = local_name_to_string(&name.local);
            let mut attr_map = IndexMap::new();
            for attr in attrs.borrow().iter() {
                attr_map.insert(
                    local_name_to_string(&attr.name.local),
                    attr.value.to_string(),
                );
            }
            let id = nodes.len();
            nodes.push(Node {
                kind: NodeKind::Element(tag),
                attrs: attr_map,
                children: Vec::new(),
                parent,
            });
            for child in handle.children.borrow().iter() {
                let cid = convert(child, nodes, Some(id));
                nodes[id].children.push(cid);
            }
            return id;
        }
        RcNodeData::ProcessingInstruction { .. } => NodeKind::Comment(String::new()),
    };

    let id = nodes.len();
    nodes.push(Node {
        kind,
        attrs: IndexMap::new(),
        children: Vec::new(),
        parent,
    });
    for child in handle.children.borrow().iter() {
        let cid = convert(child, nodes, Some(id));
        nodes[id].children.push(cid);
    }
    id
}

fn local_name_to_string(name: &LocalName) -> String {
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_finds_tags() {
        let dom = Dom::parse("<html><body><p id=\"x\">hi <b>there</b></p></body></html>");
        let ps = dom.find_all_tag_global("p");
        assert_eq!(ps.len(), 1);
        assert_eq!(
            dom.node(ps[0]).attrs.get("id").map(|s| s.as_str()),
            Some("x")
        );
        let bs = dom.find_all_tag_global("b");
        assert_eq!(bs.len(), 1);
    }

    #[test]
    fn rename_and_serialize_roundtrip() {
        let mut dom = Dom::parse("<html><body><i>hello</i></body></html>");
        let i = dom.find_first_tag_global("i").unwrap();
        dom.set_tag(i, "span");
        dom.node_mut(i)
            .attrs
            .insert("class".to_string(), "italic".to_string());
        let body = dom.find_first_tag_global("body").unwrap();
        let out = dom.serialize(body);
        assert!(out.contains("<span class=\"italic\">hello</span>"), "{out}");
    }

    #[test]
    fn prev_sibling_walks_backward() {
        let dom = Dom::parse("<html><body><p>a</p><p>b</p><p>c</p></body></html>");
        let ps = dom.find_all_tag_global("p");
        assert_eq!(ps.len(), 3);
        assert_eq!(dom.prev_sibling(ps[0]), None);
        assert_eq!(dom.prev_sibling(ps[1]), Some(ps[0]));
        assert_eq!(dom.prev_sibling(ps[2]), Some(ps[1]));
    }

    #[test]
    fn remove_promoting_children() {
        let mut dom = Dom::parse("<html><body><div><p>keep</p></div></body></html>");
        let div = dom.find_first_tag_global("div").unwrap();
        dom.remove_promoting_children(div);
        let body = dom.find_first_tag_global("body").unwrap();
        let out = dom.serialize(body);
        assert!(out.contains("<p>keep</p>"), "{out}");
        assert!(!out.contains("<div>"), "{out}");
    }
}
