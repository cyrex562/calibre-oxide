//! A minimal XML element builder.
//!
//! The Python writer builds its parts with `lxml.builder.ElementMaker`
//! and serializes them with `etree.tostring`. `roxmltree`, which the
//! rest of this crate parses with, is read-only, so the writer carries
//! its own builder. It is deliberately small: OOXML parts are written
//! with explicit prefixes and all namespaces declared on the root,
//! which is what Word itself emits, so there is no prefix bookkeeping
//! to do.

use std::fmt::Write;

/// An element's child: another element, or a run of text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Child {
    Element(Element),
    Text(String),
}

/// An XML element, with attributes in insertion order.
///
/// Attribute order matters here only for output stability — tests and
/// diffs are far easier to read when the same input yields byte-identical
/// output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    /// The tag, prefix included: `w:pPr`, `Relationship`, ...
    pub name: String,
    /// `xmlns` declarations to emit on this element, as
    /// `(prefix, uri)`. An empty prefix is the default namespace.
    pub namespaces: Vec<(String, String)>,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Child>,
}

impl Element {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            namespaces: Vec::new(),
            attrs: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Declare a namespace on this element. Use an empty prefix for the
    /// default namespace.
    pub fn ns(mut self, prefix: impl Into<String>, uri: impl Into<String>) -> Self {
        self.namespaces.push((prefix.into(), uri.into()));
        self
    }

    /// Set an attribute, builder-style.
    pub fn attr(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.set(name, value);
        self
    }

    /// Set an attribute, replacing any existing one of the same name.
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        let value = value.into();
        match self.attrs.iter_mut().find(|(n, _)| *n == name) {
            Some(slot) => slot.1 = value,
            None => self.attrs.push((name, value)),
        }
    }

    /// The value of an attribute, if set.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Append a child element, builder-style.
    pub fn with(mut self, child: Element) -> Self {
        self.children.push(Child::Element(child));
        self
    }

    /// Append a text child, builder-style.
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.children.push(Child::Text(text.into()));
        self
    }

    /// Append a child element, and hand back a mutable reference to it
    /// so it can be filled in — the shape the Python's `makeelement`
    /// helper has.
    pub fn append(&mut self, child: Element) -> &mut Element {
        self.children.push(Child::Element(child));
        match self.children.last_mut() {
            Some(Child::Element(e)) => e,
            _ => unreachable!("just pushed an element"),
        }
    }

    /// Insert a child element at `index`, shifting later children
    /// back. Port of lxml's `element.insert(index, child)`, used by
    /// `docx/writer/links.py` and `images.py` to prepend paragraphs
    /// (`body.insert(0, p)`) into an already-built `<w:body>`.
    pub fn insert(&mut self, index: usize, child: Element) {
        self.children.insert(index, Child::Element(child));
    }

    /// Move the child at `index` to the end, shifting later children
    /// forward to fill the gap. Port of lxml's `element.append(child)`
    /// where `child` is ALREADY one of `element`'s own children --
    /// unlike this port's own [`Self::append`] (always adds a NEW
    /// child), lxml's `append` re-parents an existing node, which for
    /// an element already a child of `self` is just a move. Used by
    /// `docx/writer/from_html.py`'s `Convert.write` (`body.append(
    /// body[0])`, moving the skeleton's placeholder `w:sectPr` from
    /// the front of `<w:body>` to the end, after real paragraph
    /// content has been serialized in ahead of it -- OOXML requires
    /// the document's final `w:sectPr` be `<w:body>`'s last child).
    /// A no-op if `index` is already the last child.
    pub fn move_child_to_end(&mut self, index: usize) {
        if index + 1 < self.children.len() {
            let child = self.children.remove(index);
            self.children.push(child);
        }
    }

    /// The first descendant element (not including `self`) named
    /// `name`, in document order, or `None` if there isn't one. Port
    /// of the narrow use lxml's `element.xpath('//*[local-name()="..."]')`
    /// gets put to in `docx/writer/links.py`/`images.py`: since `//`
    /// searches from the document root regardless of context node,
    /// but this writer never has more than one such element outside
    /// of `<w:body>`'s own subtree, searching just `self`'s
    /// descendants gives the same match.
    pub fn find_descendant_mut(&mut self, name: &str) -> Option<&mut Element> {
        for child in &mut self.children {
            if let Child::Element(e) = child {
                if e.name == name {
                    return Some(e);
                }
                if let Some(found) = e.find_descendant_mut(name) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Whether the element has any children at all.
    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }

    /// Number of child elements, ignoring text.
    pub fn child_count(&self) -> usize {
        self.children
            .iter()
            .filter(|c| matches!(c, Child::Element(_)))
            .count()
    }

    /// Direct child elements with the given tag.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.children.iter().filter_map(move |c| match c {
            Child::Element(e) if e.name == name => Some(e),
            _ => None,
        })
    }

    /// Serialize to XML, with the standard declaration.
    pub fn to_xml(&self) -> String {
        let mut out = String::from("<?xml version='1.0' encoding='utf-8'?>\n");
        self.write(&mut out);
        out
    }

    /// Serialize to XML without a declaration.
    pub fn to_xml_fragment(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        let _ = write!(out, "<{}", self.name);
        for (prefix, uri) in &self.namespaces {
            if prefix.is_empty() {
                let _ = write!(out, " xmlns=\"{}\"", escape_attr(uri));
            } else {
                let _ = write!(out, " xmlns:{prefix}=\"{}\"", escape_attr(uri));
            }
        }
        for (name, value) in &self.attrs {
            let _ = write!(out, " {name}=\"{}\"", escape_attr(value));
        }
        if self.children.is_empty() {
            out.push_str("/>");
            return;
        }
        out.push('>');
        for child in &self.children {
            match child {
                Child::Element(e) => e.write(out),
                Child::Text(t) => out.push_str(&escape_text(t)),
            }
        }
        let _ = write!(out, "</{}>", self.name);
    }
}

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attr(s: &str) -> String {
    escape_text(s)
        .replace('"', "&quot;")
        .replace('\t', "&#9;")
        .replace('\n', "&#10;")
        .replace('\r', "&#13;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use roxmltree::Document;

    #[test]
    fn an_empty_element_self_closes() {
        assert_eq!(Element::new("w:br").to_xml_fragment(), "<w:br/>");
    }

    #[test]
    fn attributes_keep_insertion_order_and_replace_by_name() {
        let mut e = Element::new("w:sz").attr("w:val", "22").attr("w:id", "1");
        e.set("w:val", "24");
        assert_eq!(
            e.to_xml_fragment(),
            r#"<w:sz w:val="24" w:id="1"/>"#,
            "replacing must not move the attribute"
        );
        assert_eq!(e.get("w:val"), Some("24"));
        assert_eq!(e.get("w:nope"), None);
    }

    #[test]
    fn namespaces_are_declared_where_asked() {
        let e = Element::new("Relationships").ns("", "http://example.com/rel");
        assert_eq!(
            e.to_xml_fragment(),
            r#"<Relationships xmlns="http://example.com/rel"/>"#
        );
        let e = Element::new("w:p").ns("w", "http://example.com/w");
        assert!(e.to_xml_fragment().starts_with(r#"<w:p xmlns:w="#));
    }

    #[test]
    fn text_and_markup_characters_are_escaped() {
        let e = Element::new("w:t")
            .ns("w", "http://example.com/w")
            .attr("note", r#"a "b" & <c>"#)
            .with_text("1 < 2 & 3 > 0");
        let xml = e.to_xml();
        assert!(xml.contains("1 &lt; 2 &amp; 3 &gt; 0"), "{xml}");
        assert!(
            xml.contains(r#"note="a &quot;b&quot; &amp; &lt;c&gt;""#),
            "{xml}"
        );
        // The real check: it has to parse back to what went in.
        let doc = Document::parse(&xml).expect("reparses");
        let root = doc.root_element();
        assert_eq!(root.text(), Some("1 < 2 & 3 > 0"));
        assert_eq!(root.attribute("note"), Some(r#"a "b" & <c>"#));
    }

    #[test]
    fn newlines_and_tabs_in_attributes_survive_the_round_trip() {
        // Unescaped, an XML parser normalises these to spaces.
        let e = Element::new("a").attr("v", "one\ttwo\nthree");
        let xml = e.to_xml();
        let doc = Document::parse(&xml).expect("reparses");
        assert_eq!(doc.root_element().attribute("v"), Some("one\ttwo\nthree"));
    }

    #[test]
    fn nesting_builds_the_tree_in_order() {
        let mut body = Element::new("w:body");
        let p = body.append(Element::new("w:p"));
        p.append(Element::new("w:r")).append(Element::new("w:t"));
        body.append(Element::new("w:sectPr"));

        assert_eq!(
            body.to_xml_fragment(),
            "<w:body><w:p><w:r><w:t/></w:r></w:p><w:sectPr/></w:body>"
        );
        assert_eq!(body.child_count(), 2);
        assert_eq!(body.children_named("w:p").count(), 1);
        assert!(!body.is_empty());
    }

    #[test]
    fn insert_prepends_and_shifts_later_children_back() {
        let mut body = Element::new("w:body");
        body.append(Element::new("w:sectPr"));
        body.insert(0, Element::new("w:p").attr("w:id", "2"));
        body.insert(0, Element::new("w:p").attr("w:id", "1"));
        assert_eq!(
            body.to_xml_fragment(),
            r#"<w:body><w:p w:id="1"/><w:p w:id="2"/><w:sectPr/></w:body>"#
        );
    }

    #[test]
    fn move_child_to_end_moves_the_first_child_after_later_ones() {
        let mut body = Element::new("w:body");
        body.append(Element::new("w:sectPr"));
        body.insert(0, Element::new("w:p").attr("w:id", "1"));
        body.insert(1, Element::new("w:p").attr("w:id", "2"));
        // At this point: [p1, p2, sectPr]. Moving index 0 (which is
        // now p1, not sectPr) checks the move is by CURRENT position,
        // not by content -- exactly matching lxml's index semantics.
        body.move_child_to_end(0);
        assert_eq!(
            body.to_xml_fragment(),
            r#"<w:body><w:p w:id="2"/><w:sectPr/><w:p w:id="1"/></w:body>"#
        );
    }

    #[test]
    fn move_child_to_end_on_the_last_child_is_a_no_op() {
        let mut body = Element::new("w:body").with(Element::new("w:p"));
        body.move_child_to_end(0);
        assert_eq!(body.to_xml_fragment(), "<w:body><w:p/></w:body>");
    }

    #[test]
    fn find_descendant_mut_finds_the_first_match_in_document_order() {
        let mut body = Element::new("w:body");
        let p = body.append(Element::new("w:p"));
        p.append(Element::new("w:pPr"))
            .append(Element::new("w:pageBreakBefore").attr("w:val", "off"));
        body.append(
            Element::new("w:p").with(Element::new("w:pageBreakBefore").attr("w:val", "off")),
        );
        let found = body
            .find_descendant_mut("w:pageBreakBefore")
            .expect("first pageBreakBefore");
        found.set("w:val", "on");
        assert_eq!(body.children_named("w:p").count(), 2);
        let xml = body.to_xml_fragment();
        assert_eq!(
            xml.matches(r#"w:val="on""#).count(),
            1,
            "only the first match should have been mutated: {xml}"
        );
    }

    #[test]
    fn find_descendant_mut_returns_none_when_absent() {
        let mut body = Element::new("w:body").with(Element::new("w:p"));
        assert!(body.find_descendant_mut("w:sectPr").is_none());
    }

    #[test]
    fn the_declaration_is_emitted_only_by_to_xml() {
        let e = Element::new("a");
        assert!(e
            .to_xml()
            .starts_with("<?xml version='1.0' encoding='utf-8'?>\n"));
        assert!(!e.to_xml_fragment().starts_with("<?xml"));
    }
}
