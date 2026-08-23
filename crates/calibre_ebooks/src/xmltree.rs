//! A small, mutable, namespace-aware XML arena for "real", strict XML
//! documents -- as opposed to tag-soup HTML5, see [`crate::dom`]. Built
//! for the OPF/NCX/OCF documents `oeb::polish::container` edits in
//! place (OPF, NCX, `META-INF/container.xml`,
//! `META-INF/encryption.xml`); reused as of the docx HTML-generation
//! port (issue #130) for OOXML's `word/document.xml` and
//! `word/numbering.xml`, whose source-tree mutation needs (merged-cell
//! removal in `docx::tables`, synthetic-element insertion in the
//! not-yet-ported `fields.py`/`index.py`) are the same shape. It lived
//! under `oeb::polish::xmltree` until that reuse made the location
//! misleading; it was moved to the crate root without any behavior
//! change. Nothing in this module is actually OPF-specific -- the
//! namespace URI/prefix table any query needs is always supplied by
//! the caller (see [`Xml::opf_xpath`]'s `ns` parameter) -- the name
//! just predates its reuse.
//!
//! `container.py`'s heavy tree work is split across two very different
//! kinds of documents:
//!
//! - XHTML/HTML content documents, which Python parses leniently
//!   (`parsing.py`'s `parse`/`parse_html5`, falling back to tag-soup on
//!   any strict-XML failure). That side of the port reuses
//!   [`crate::dom`] directly -- see `oeb::polish::parsing`.
//! - OPF-family documents, which Python parses with a **strict**,
//!   namespace-aware XML parser (`safe_xml_fromstring`) and then
//!   mutates in place via `lxml.etree` (`makeelement`, `.set`/`.get`,
//!   `.xpath('//opf:manifest/opf:item[...]')`, `.remove()`, `.insert()`,
//!   reading/writing `.text`).
//!
//! [`crate::dom::Dom`] is HTML5-shaped (built on `html5ever`'s HTML
//! parser) and is not a fit for the second case: HTML5 parsing rules
//! only treat a fixed list of *void* elements (`br`, `img`, `meta`, ...)
//! as self-closing, so an OPF `<item .../>` -- not on that list -- would
//! be parsed as an *unclosed* element that silently swallows every
//! subsequent manifest entry as a descendant. Real OPF/NCX/OCF XML needs
//! a real XML parser. This module provides that: a mutable arena built
//! by converting a one-shot [`roxmltree::Document`] parse into an owned
//! `Vec<XmlNode>` (the same "parse once with a real parser, then mutate
//! a simpler arena" shape `crate::dom` uses for HTML, just with
//! `roxmltree` standing in for `html5ever`).
//!
//! # Namespace handling (scope note)
//!
//! Attributes in every document this module needs to handle (OPF, NCX,
//! OCF container/encryption XML) are, in practice, always unprefixed
//! (`href`, `id`, `media-type`, ...) -- which is also correct XML
//! semantics: a default `xmlns` does *not* apply to attributes, only
//! elements. So attributes are stored simply by their local name; this
//! is not a shortcut, it matches the real documents byte-for-byte.
//!
//! Elements *do* need namespace awareness (`opf:manifest` vs. a bare
//! `<manifest>` under a default `xmlns` both mean the same thing, and
//! `container.py`'s `opf_xpath` queries by resolved namespace URI, not
//! literal prefix). Namespace *declarations* are captured once, from
//! whatever is in scope at the document's root element (true for every
//! real-world OPF this crate needs to read, and for anything this crate
//! itself generates) and re-emitted on the root element when
//! serializing; see [`Xml::doc_namespaces`].
//!
//! # Whitespace / pretty-printing (scope note)
//!
//! `insert_self_closing`/`remove_from_xml` in Python fuss over
//! `.text`/`.tail` redistribution purely for cosmetic indentation in the
//! serialized OPF. This arena represents whitespace as ordinary sibling
//! [`XmlNodeKind::Text`] nodes rather than out-of-band `.text`/`.tail`
//! attributes, which makes *removal* trivially correct (surrounding text
//! nodes simply stay where they are) and makes *insertion* a matter of
//! picking the right position among sibling elements -- see
//! [`Xml::insert_element`]. This does not attempt to reproduce lxml's
//! exact indentation byte-for-byte; the output is always well-formed and
//! structurally correct, which is what every caller in this crate
//! depends on.

use std::collections::HashMap;

use anyhow::{Context, Result};
use indexmap::IndexMap;

pub type XmlNodeId = usize;

#[derive(Debug, Clone)]
pub enum XmlNodeKind {
    /// The arena's single root pseudo-node (mirrors `roxmltree`'s
    /// document node); its children are the top-level comments/PIs and
    /// the single root element.
    Document,
    Element {
        /// Local (unprefixed) tag name, e.g. `"manifest"`, `"item"`,
        /// `"language"`.
        local: String,
        /// Resolved namespace URI, if any.
        namespace: Option<String>,
    },
    Text(String),
    Comment(String),
}

#[derive(Debug, Clone)]
pub struct XmlNode {
    pub kind: XmlNodeKind,
    /// Attribute local-name -> value, insertion-ordered. See the module
    /// docs for why namespaced attributes are not distinguished.
    pub attrs: IndexMap<String, String>,
    pub children: Vec<XmlNodeId>,
    pub parent: Option<XmlNodeId>,
    /// 1-based source line number, when parsed (mirrors lxml's
    /// `elem.sourceline`, used by `Container::iterlinks` for error
    /// reporting). `None` for freshly created elements.
    pub sourceline: Option<u32>,
}

/// The mutable XML document. Cheap to clone the `XmlNodeId`s it hands
/// out (they're just `usize` indices into `nodes`) but the arena itself
/// owns all node data.
#[derive(Debug, Clone)]
pub struct Xml {
    nodes: Vec<XmlNode>,
    pub root: XmlNodeId,
    /// Namespace declarations in scope at the document's root element:
    /// prefix (`None` = default namespace) -> URI. Re-emitted verbatim
    /// as `xmlns`/`xmlns:*` attributes on the root element when
    /// serializing. See the module-level scope note.
    pub doc_namespaces: IndexMap<Option<String>, String>,
}

impl Xml {
    /// Parses `data` as a strict XML document. Port of
    /// `container.py`'s `ContainerBase.parse_xml` (which wraps
    /// `calibre.utils.xml_parse.safe_xml_fromstring`), minus the
    /// `resolve_entities`/DTD handling lxml does internally --
    /// `roxmltree` resolves the five predefined XML entities and
    /// numeric character references itself, which covers every
    /// document this module needs to read (OPF/NCX/OCF XML never
    /// legitimately define custom DTD entities).
    pub fn parse(data: &str) -> Result<Xml> {
        let doc = roxmltree::Document::parse(data).context("Failed to parse XML")?;
        let mut nodes = vec![XmlNode {
            kind: XmlNodeKind::Document,
            attrs: IndexMap::new(),
            children: Vec::new(),
            parent: None,
            sourceline: None,
        }];
        let root_id: XmlNodeId = 0;

        let mut doc_namespaces = IndexMap::new();
        for ns in doc.root_element().namespaces() {
            doc_namespaces.insert(ns.name().map(|s| s.to_string()), ns.uri().to_string());
        }

        for child in doc.root().children() {
            let cid = convert(&doc, &child, &mut nodes, Some(root_id));
            nodes[root_id].children.push(cid);
        }

        Ok(Xml {
            nodes,
            root: root_id,
            doc_namespaces,
        })
    }

    pub fn node(&self, id: XmlNodeId) -> &XmlNode {
        &self.nodes[id]
    }

    pub fn node_mut(&mut self, id: XmlNodeId) -> &mut XmlNode {
        &mut self.nodes[id]
    }

    pub fn root_element(&self) -> Option<XmlNodeId> {
        self.nodes[self.root]
            .children
            .iter()
            .copied()
            .find(|&c| matches!(self.nodes[c].kind, XmlNodeKind::Element { .. }))
    }

    pub fn local_name(&self, id: XmlNodeId) -> Option<&str> {
        match &self.nodes[id].kind {
            XmlNodeKind::Element { local, .. } => Some(local.as_str()),
            _ => None,
        }
    }

    pub fn namespace(&self, id: XmlNodeId) -> Option<&str> {
        match &self.nodes[id].kind {
            XmlNodeKind::Element { namespace, .. } => namespace.as_deref(),
            _ => None,
        }
    }

    pub fn get_attr(&self, id: XmlNodeId, name: &str) -> Option<&str> {
        self.nodes[id].attrs.get(name).map(|s| s.as_str())
    }

    pub fn has_attr(&self, id: XmlNodeId, name: &str) -> bool {
        self.nodes[id].attrs.contains_key(name)
    }

    pub fn set_attr(&mut self, id: XmlNodeId, name: &str, value: impl Into<String>) {
        self.nodes[id].attrs.insert(name.to_string(), value.into());
    }

    pub fn remove_attr(&mut self, id: XmlNodeId, name: &str) {
        self.nodes[id].attrs.shift_remove(name);
    }

    pub fn parent(&self, id: XmlNodeId) -> Option<XmlNodeId> {
        self.nodes[id].parent
    }

    pub fn children(&self, id: XmlNodeId) -> &[XmlNodeId] {
        &self.nodes[id].children
    }

    /// Direct `Element` children of `id`, in document order (lxml's
    /// `list(elem)` / `len(elem)` counts only these, never text nodes).
    pub fn element_children(&self, id: XmlNodeId) -> Vec<XmlNodeId> {
        self.nodes[id]
            .children
            .iter()
            .copied()
            .filter(|&c| matches!(self.nodes[c].kind, XmlNodeKind::Element { .. }))
            .collect()
    }

    pub fn index_in_parent(&self, id: XmlNodeId) -> Option<usize> {
        let parent = self.nodes[id].parent?;
        self.nodes[parent].children.iter().position(|&c| c == id)
    }

    /// The element's own leading text (lxml's `elem.text`): the first
    /// child, if it is a `Text` node.
    pub fn element_text(&self, id: XmlNodeId) -> Option<&str> {
        let first = *self.nodes[id].children.first()?;
        match &self.nodes[first].kind {
            XmlNodeKind::Text(t) => Some(t.as_str()),
            _ => None,
        }
    }

    /// Sets (creating if absent) the element's leading text child.
    pub fn set_element_text(&mut self, id: XmlNodeId, text: impl Into<String>) {
        let text = text.into();
        if let Some(&first) = self.nodes[id].children.first() {
            if let XmlNodeKind::Text(t) = &mut self.nodes[first].kind {
                *t = text;
                return;
            }
        }
        let tid = self.new_text(&text);
        self.nodes[tid].parent = Some(id);
        self.nodes[id].children.insert(0, tid);
    }

    /// All `Text` descendant content concatenated together
    /// (`elem.xpath('descendant::text()')`-style usage), used for
    /// error-message contexts, not by the ported files' hot paths.
    pub fn text_content(&self, id: XmlNodeId) -> String {
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
            if let XmlNodeKind::Text(t) = &self.nodes[n].kind {
                out.push_str(t);
            }
        }
        out
    }

    pub fn new_element(&mut self, local: &str, namespace: Option<&str>) -> XmlNodeId {
        self.nodes.push(XmlNode {
            kind: XmlNodeKind::Element {
                local: local.to_string(),
                namespace: namespace.map(|s| s.to_string()),
            },
            attrs: IndexMap::new(),
            children: Vec::new(),
            parent: None,
            sourceline: None,
        });
        self.nodes.len() - 1
    }

    pub fn new_text(&mut self, text: &str) -> XmlNodeId {
        self.nodes.push(XmlNode {
            kind: XmlNodeKind::Text(text.to_string()),
            attrs: IndexMap::new(),
            children: Vec::new(),
            parent: None,
            sourceline: None,
        });
        self.nodes.len() - 1
    }

    /// Ensures `uri` has a declared prefix (registering `prefix` for it
    /// if it isn't already declared under any prefix). Port of the
    /// effect of lxml's `nsmap=` kwarg on `makeelement` -- callers that
    /// need a specific literal prefix on newly created elements (e.g.
    /// `opf.py`'s `set_guide_item`, which explicitly passes
    /// `nsmap={'opf': OPF_NAMESPACES['opf']}`) call this before/along
    /// with [`Xml::new_element`].
    pub fn ensure_namespace_declared(&mut self, prefix: Option<&str>, uri: &str) {
        if self.doc_namespaces.values().any(|v| v == uri) {
            return;
        }
        self.doc_namespaces
            .insert(prefix.map(|s| s.to_string()), uri.to_string());
    }

    /// Detaches `id` from its parent's child list. The node itself
    /// (and its subtree) stays alive in the arena so other `XmlNodeId`s
    /// referencing it remain valid; equivalent in effect to lxml
    /// `parent.remove(elem)`. Because whitespace is represented as
    /// ordinary sibling text nodes here (not out-of-band `.tail`
    /// attributes -- see the module scope note), this is *already* what
    /// `container.py`'s `remove_from_xml` exists to do by hand in lxml:
    /// the surrounding text nodes are untouched and stay correctly
    /// positioned relative to whatever now-adjacent siblings remain.
    pub fn detach(&mut self, id: XmlNodeId) {
        if let Some(parent) = self.nodes[id].parent.take() {
            self.nodes[parent].children.retain(|&c| c != id);
        }
    }

    /// Inserts `item` (an `Element`) as the `index`-th *element* child
    /// of `parent` (append if `index` is `None`), matching Python
    /// `insert_self_closing`'s indexing (which counts only elements,
    /// per lxml's `parent.index`/`parent.insert` semantics). See the
    /// module scope note on why this does not also try to reproduce
    /// lxml's `.text`/`.tail` cosmetic redistribution.
    pub fn insert_element(&mut self, parent: XmlNodeId, item: XmlNodeId, index: Option<usize>) {
        let elem_children = self.element_children(parent);
        let n = elem_children.len();
        let elem_index = index.unwrap_or(n).min(n);
        let raw_pos = if elem_index == 0 {
            0
        } else {
            self.index_in_parent(elem_children[elem_index - 1])
                .map(|p| p + 1)
                .unwrap_or(self.nodes[parent].children.len())
        };
        self.detach(item);
        self.nodes[item].parent = Some(parent);
        let raw_pos = raw_pos.min(self.nodes[parent].children.len());
        self.nodes[parent].children.insert(raw_pos, item);
    }

    /// Serializes the whole document to UTF-8 XML bytes, matching
    /// `container.py`'s `serialize_item` (which calls `oeb.base.serialize`
    /// on the OPF/NCX tree). Always emits the `<?xml ...?>` prolog and a
    /// self-closing tag for any element with no children, mirroring
    /// lxml's default output style.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
        for &c in self.children(self.root) {
            self.serialize_node(c, &mut out, c == self.nodes[self.root].children[0]);
        }
        out.into_bytes()
    }

    fn serialize_node(&self, id: XmlNodeId, out: &mut String, is_root_element: bool) {
        match &self.nodes[id].kind {
            XmlNodeKind::Document => {}
            XmlNodeKind::Text(t) => {
                out.push_str(&crate::xml_util::prepare_string_for_xml(t, false))
            }
            XmlNodeKind::Comment(t) => {
                out.push_str("<!--");
                out.push_str(t);
                out.push_str("-->");
            }
            XmlNodeKind::Element { local, namespace } => {
                let qname = self.qualified_name(namespace.as_deref(), local);
                out.push('<');
                out.push_str(&qname);
                if is_root_element {
                    for (prefix, uri) in &self.doc_namespaces {
                        out.push(' ');
                        match prefix {
                            Some(p) => {
                                out.push_str("xmlns:");
                                out.push_str(p);
                            }
                            None => out.push_str("xmlns"),
                        }
                        out.push_str("=\"");
                        out.push_str(uri);
                        out.push('"');
                    }
                }
                for (k, v) in &self.nodes[id].attrs {
                    out.push(' ');
                    out.push_str(k);
                    out.push_str("=\"");
                    out.push_str(&crate::xml_util::prepare_string_for_xml(v, true));
                    out.push('"');
                }
                if self.nodes[id].children.is_empty() {
                    out.push_str(" />");
                } else {
                    out.push('>');
                    for &c in &self.nodes[id].children {
                        self.serialize_node(c, out, false);
                    }
                    out.push_str("</");
                    out.push_str(&qname);
                    out.push('>');
                }
            }
        }
    }

    /// Resolves the literal `prefix:local` (or bare `local`) rendering
    /// for an element, given its resolved namespace URI, by looking it
    /// up in [`Xml::doc_namespaces`]. Elements with no namespace, or
    /// whose namespace matches the current default, are unprefixed.
    fn qualified_name(&self, namespace: Option<&str>, local: &str) -> String {
        match namespace {
            None => local.to_string(),
            Some(uri) => {
                if self.doc_namespaces.get(&None).map(|s| s.as_str()) == Some(uri) {
                    return local.to_string();
                }
                match self.doc_namespaces.iter().find(|(_, u)| u.as_str() == uri) {
                    Some((Some(prefix), _)) => format!("{prefix}:{local}"),
                    _ => local.to_string(),
                }
            }
        }
    }

    /// A narrow XPath subset covering exactly the shapes
    /// `container.py`/`opf.py` use against the OPF tree:
    /// `//prefix:tag(/prefix:tag)*` optionally followed on the final
    /// step by `[@attr]`/`[@attr1 and @attr2 ...]`, where each clause is
    /// either bare attribute *existence* (`@attr`) or an exact-value
    /// equality test (`@attr="value"`) -- no functions/other axes. `*`
    /// matches any element regardless of namespace. This is *not* a
    /// general XPath engine (calibre's own `opf_xpath` call sites never
    /// need one); building one is out of scope, see
    /// `docs/AGENT_PORTING_GUIDE.md`'s "simplify deeply nested
    /// hierarchies" guidance.
    ///
    /// The first step searches descendants of the document root
    /// (`//` semantics); every subsequent step walks direct element
    /// children only (`/` semantics), matching how all real call sites
    /// are shaped (`//opf:manifest/opf:item[...]` finds `opf:manifest`
    /// anywhere, then its direct `opf:item` children).
    pub fn opf_xpath(&self, expr: &str, ns: &HashMap<&str, &str>) -> Vec<XmlNodeId> {
        let body = expr.strip_prefix("//").unwrap_or(expr);
        let raw_steps: Vec<&str> = split_steps(body);
        if raw_steps.is_empty() {
            return Vec::new();
        }
        let mut current: Vec<XmlNodeId> = vec![self.root];
        let last_idx = raw_steps.len() - 1;
        for (i, raw_step) in raw_steps.iter().enumerate() {
            let (tag_part, predicate) = split_predicate(raw_step);
            let mut next = Vec::new();
            if i == 0 {
                for &id in &current {
                    self.collect_descendants_matching(id, tag_part, ns, &mut next);
                }
            } else {
                for &parent in &current {
                    for &child in &self.nodes[parent].children {
                        if self.tag_matches(child, tag_part, ns) {
                            next.push(child);
                        }
                    }
                }
            }
            if i == last_idx {
                if let Some(preds) = &predicate {
                    next.retain(|&id| preds.iter().all(|p| p.matches(self, id)));
                }
            }
            current = next;
        }
        current
    }

    fn collect_descendants_matching(
        &self,
        id: XmlNodeId,
        tag_part: &str,
        ns: &HashMap<&str, &str>,
        out: &mut Vec<XmlNodeId>,
    ) {
        for &c in &self.nodes[id].children {
            if self.tag_matches(c, tag_part, ns) {
                out.push(c);
            }
            self.collect_descendants_matching(c, tag_part, ns, out);
        }
    }

    fn tag_matches(&self, id: XmlNodeId, tag_part: &str, ns: &HashMap<&str, &str>) -> bool {
        let (local, namespace) = match &self.nodes[id].kind {
            XmlNodeKind::Element { local, namespace } => (local.as_str(), namespace.as_deref()),
            _ => return false,
        };
        if tag_part == "*" {
            return true;
        }
        match tag_part.split_once(':') {
            Some((prefix, want_local)) => {
                let want_uri = ns.get(prefix).copied();
                want_local == local && want_uri == namespace
            }
            None => tag_part == local && namespace.is_none(),
        }
    }
}

/// Splits an XPath body on `/` at bracket depth 0 only, so a predicate
/// value that itself contains a literal `/` (overwhelmingly common for
/// this crate's real queries -- MIME types like `text/css`,
/// `image/png`, `application/xhtml+xml`) isn't mistaken for a step
/// separator. `[`/`]` never legitimately nest in the predicate shapes
/// this engine supports (see [`Xml::opf_xpath`]'s docs), so plain depth
/// tracking is sufficient.
fn split_steps(body: &str) -> Vec<&str> {
    let mut steps = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in body.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            '/' if depth == 0 => {
                steps.push(&body[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    steps.push(&body[start..]);
    steps
}

/// One `[@attr]`/`[@attr="value"]` predicate clause.
enum AttrPredicate<'a> {
    /// `@attr`: the attribute is present, regardless of value.
    Exists(&'a str),
    /// `@attr="value"`: the attribute is present and its value matches
    /// exactly (quotes -- `"..."` or `'...'` -- stripped).
    Equals(&'a str, &'a str),
}

impl AttrPredicate<'_> {
    fn matches(&self, xml: &Xml, id: XmlNodeId) -> bool {
        match self {
            AttrPredicate::Exists(name) => xml.has_attr(id, name),
            AttrPredicate::Equals(name, value) => xml.get_attr(id, name) == Some(*value),
        }
    }
}

/// Splits `item[@href and @media-type]` into `("item", Some([Exists("href"),
/// Exists("media-type")]))`, `item[@type="cover"]` into `("item",
/// Some([Equals("type", "cover")]))`, or `item` into `("item", None)`.
fn split_predicate(step: &str) -> (&str, Option<Vec<AttrPredicate<'_>>>) {
    match step.find('[') {
        None => (step, None),
        Some(open) => {
            let tag = &step[..open];
            let inner = step[open + 1..].trim_end_matches(']');
            let attrs = inner
                .split(" and ")
                .filter_map(|p| {
                    let p = p.trim().strip_prefix('@')?;
                    Some(match p.split_once('=') {
                        Some((name, value)) => {
                            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
                            AttrPredicate::Equals(name.trim(), value)
                        }
                        None => AttrPredicate::Exists(p),
                    })
                })
                .collect();
            (tag, Some(attrs))
        }
    }
}

fn convert(
    doc: &roxmltree::Document,
    node: &roxmltree::Node,
    nodes: &mut Vec<XmlNode>,
    parent: Option<XmlNodeId>,
) -> XmlNodeId {
    if node.is_element() {
        let local = node.tag_name().name().to_string();
        let namespace = node.tag_name().namespace().map(|s| s.to_string());
        let mut attrs = IndexMap::new();
        for attr in node.attributes() {
            attrs.insert(attr.name().to_string(), attr.value().to_string());
        }
        let sourceline = Some(doc.text_pos_at(node.range().start).row);
        let id = nodes.len();
        nodes.push(XmlNode {
            kind: XmlNodeKind::Element { local, namespace },
            attrs,
            children: Vec::new(),
            parent,
            sourceline,
        });
        for child in node.children() {
            let cid = convert(doc, &child, nodes, Some(id));
            nodes[id].children.push(cid);
        }
        id
    } else if node.is_text() {
        let id = nodes.len();
        nodes.push(XmlNode {
            kind: XmlNodeKind::Text(node.text().unwrap_or("").to_string()),
            attrs: IndexMap::new(),
            children: Vec::new(),
            parent,
            sourceline: None,
        });
        id
    } else if node.is_comment() {
        let id = nodes.len();
        nodes.push(XmlNode {
            kind: XmlNodeKind::Comment(node.text().unwrap_or("").to_string()),
            attrs: IndexMap::new(),
            children: Vec::new(),
            parent,
            sourceline: None,
        });
        id
    } else {
        // Processing instructions and other node kinds are dropped --
        // none of the documents this module handles carry meaningful
        // ones (OPF/NCX/OCF XML never legitimately have PIs beyond the
        // XML declaration itself, which `roxmltree` doesn't expose as a
        // node at all).
        let id = nodes.len();
        nodes.push(XmlNode {
            kind: XmlNodeKind::Comment(String::new()),
            attrs: IndexMap::new(),
            children: Vec::new(),
            parent,
            sourceline: None,
        });
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opf_ns() -> HashMap<&'static str, &'static str> {
        let mut m = HashMap::new();
        m.insert("opf", crate::oeb::constants::OPF2_NS);
        m.insert("dc", crate::oeb::constants::DC11_NS);
        m
    }

    const SAMPLE: &str = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata>
    <dc:title>Hello</dc:title>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
    <item id="css" href="style.css" media-type="text/css"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
  </spine>
</package>"#;

    #[test]
    fn parses_and_finds_elements_by_prefix() {
        let xml = Xml::parse(SAMPLE).unwrap();
        let ns = opf_ns();
        let items = xml.opf_xpath("//opf:manifest/opf:item[@href and @id]", &ns);
        assert_eq!(items.len(), 2);
        assert_eq!(xml.get_attr(items[0], "href"), Some("chap1.html"));

        let langs = xml.opf_xpath("//dc:language", &ns);
        assert_eq!(langs.len(), 1);
        assert_eq!(xml.element_text(langs[0]), Some("en"));
    }

    #[test]
    fn value_equality_predicate() {
        let xml = Xml::parse(SAMPLE).unwrap();
        let ns = opf_ns();
        let css_only = xml.opf_xpath(r#"//opf:manifest/opf:item[@media-type="text/css"]"#, &ns);
        assert_eq!(css_only.len(), 1);
        assert_eq!(xml.get_attr(css_only[0], "id"), Some("css"));

        let combined = xml.opf_xpath(r#"//opf:manifest/opf:item[@id="c1" and @href]"#, &ns);
        assert_eq!(combined.len(), 1);
        assert_eq!(xml.get_attr(combined[0], "href"), Some("chap1.html"));

        let none = xml.opf_xpath(r#"//opf:manifest/opf:item[@media-type="image/png"]"#, &ns);
        assert!(none.is_empty());
    }

    #[test]
    fn wildcard_and_existence_predicate() {
        let xml = Xml::parse(SAMPLE).unwrap();
        let ns = opf_ns();
        let with_id = xml.opf_xpath("//*[@id]", &ns);
        // package has no id, but two <item>s and nothing else do.
        assert_eq!(with_id.len(), 2);
    }

    #[test]
    fn round_trips_through_serialize_and_reparse() {
        let xml = Xml::parse(SAMPLE).unwrap();
        let bytes = xml.serialize();
        let text = String::from_utf8(bytes).unwrap();
        let reparsed = Xml::parse(&text).unwrap();
        let ns = opf_ns();
        assert_eq!(reparsed.opf_xpath("//opf:manifest/opf:item", &ns).len(), 2);
        assert_eq!(reparsed.opf_xpath("//dc:title", &ns).len(), 1);
    }

    #[test]
    fn insert_and_detach_element() {
        let mut xml = Xml::parse(SAMPLE).unwrap();
        let ns = opf_ns();
        let manifest = xml.opf_xpath("//opf:manifest", &ns)[0];
        let new_item = xml.new_element("item", Some(crate::oeb::constants::OPF2_NS));
        xml.set_attr(new_item, "id", "c2");
        xml.set_attr(new_item, "href", "chap2.html");
        xml.insert_element(manifest, new_item, None);
        assert_eq!(xml.opf_xpath("//opf:manifest/opf:item", &ns).len(), 3);

        xml.detach(new_item);
        assert_eq!(xml.opf_xpath("//opf:manifest/opf:item", &ns).len(), 2);
    }

    #[test]
    fn set_attr_and_get_attr_roundtrip() {
        let mut xml = Xml::parse(SAMPLE).unwrap();
        let ns = opf_ns();
        let items = xml.opf_xpath("//opf:manifest/opf:item[@id]", &ns);
        xml.set_attr(items[0], "media-type", "application/xhtml+xml");
        assert_eq!(
            xml.get_attr(items[0], "media-type"),
            Some("application/xhtml+xml")
        );
        xml.remove_attr(items[0], "media-type");
        assert_eq!(xml.get_attr(items[0], "media-type"), None);
    }
}
