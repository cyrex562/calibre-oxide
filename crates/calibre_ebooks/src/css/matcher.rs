//! The [`Element`] trait -- this crate's tree-agnostic seam for selector
//! matching -- plus [`Select`], a small per-document index mirroring
//! Python's `css_selectors.Select`. See the `css`/`selector` module docs
//! for what selector syntax is supported.

use crate::mobi::dom::{Dom, NodeId};
use crate::oeb::polish::xmltree::{Xml, XmlNodeId, XmlNodeKind};

use super::selector::{AttrSelector, Combinator, Selector, SelectorList};

/// The minimal element-tree surface selector matching needs: a tag name,
/// attribute lookup, and a parent link. Implemented for
/// [`crate::mobi::dom::Dom`] (XHTML content documents) and
/// [`crate::oeb::polish::xmltree::Xml`] (OPF/NCX-shaped documents) node
/// references -- see the `css` module docs for why this trait exists
/// instead of adopting Servo's `selectors::Element`.
pub trait Element: Copy {
    fn local_name(&self) -> Option<String>;
    fn get_attr(&self, name: &str) -> Option<String>;
    fn parent(&self) -> Option<Self>;

    fn class_list(&self) -> Vec<String> {
        self.get_attr("class")
            .map(|c| c.split_whitespace().map(|s| s.to_string()).collect())
            .unwrap_or_default()
    }
}

/// An [`Element`] over [`Dom`] (XHTML content documents).
#[derive(Clone, Copy)]
pub struct DomElement<'a> {
    pub dom: &'a Dom,
    pub id: NodeId,
}

impl<'a> Element for DomElement<'a> {
    fn local_name(&self) -> Option<String> {
        self.dom.tag(self.id).map(|s| s.to_string())
    }

    fn get_attr(&self, name: &str) -> Option<String> {
        self.dom.node(self.id).attrs.get(name).cloned()
    }

    fn parent(&self) -> Option<Self> {
        self.dom
            .parent(self.id)
            .map(|id| DomElement { dom: self.dom, id })
    }
}

/// Every `Element` node in `dom`, in document order -- port of what
/// Python builds `css_selectors.Select(root)` from.
pub fn dom_elements(dom: &Dom) -> Vec<DomElement<'_>> {
    dom.preorder_elements(dom.root)
        .into_iter()
        .map(|id| DomElement { dom, id })
        .collect()
}

/// An [`Element`] over [`Xml`] (OPF/NCX-shaped documents).
#[derive(Clone, Copy)]
pub struct XmlElement<'a> {
    pub xml: &'a Xml,
    pub id: XmlNodeId,
}

impl<'a> Element for XmlElement<'a> {
    fn local_name(&self) -> Option<String> {
        self.xml.local_name(self.id).map(|s| s.to_string())
    }

    fn get_attr(&self, name: &str) -> Option<String> {
        self.xml.get_attr(self.id, name).map(|s| s.to_string())
    }

    fn parent(&self) -> Option<Self> {
        self.xml
            .parent(self.id)
            .map(|id| XmlElement { xml: self.xml, id })
    }
}

/// Every `Element` node in `xml`, in document order.
pub fn xml_elements(xml: &Xml) -> Vec<XmlElement<'_>> {
    let mut out = Vec::new();
    fn walk(xml: &Xml, id: XmlNodeId, out: &mut Vec<XmlNodeId>) {
        for &c in xml.children(id) {
            if matches!(xml.node(c).kind, XmlNodeKind::Element { .. }) {
                out.push(c);
            }
            walk(xml, c, out);
        }
    }
    let mut ids = Vec::new();
    walk(xml, xml.root, &mut ids);
    for id in ids {
        out.push(XmlElement { xml, id });
    }
    out
}

/// Matches one [`Selector`] (a single, comma-free selector, e.g. `div >
/// p.a`) against `elem`.
pub fn selector_matches<E: Element>(selector: &Selector, elem: E) -> bool {
    match_compounds(&selector.compounds, elem)
}

fn match_compounds<E: Element>(compounds: &[super::selector::CompoundSelector], elem: E) -> bool {
    let (last, rest) = compounds
        .split_last()
        .expect("a selector always has >=1 compound");
    if !simple_matches(&last.simple, elem) {
        return false;
    }
    if rest.is_empty() {
        return true;
    }
    // `rest`'s own last compound's `combinator` connects it to `last`,
    // the compound just matched against `elem`.
    let combinator = rest
        .last()
        .and_then(|c| c.combinator)
        .expect("every non-rightmost compound has a combinator");
    match combinator {
        Combinator::Child => match elem.parent() {
            Some(parent) => match_compounds(rest, parent),
            None => false,
        },
        Combinator::Descendant => {
            let mut cur = elem.parent();
            while let Some(p) = cur {
                if match_compounds(rest, p) {
                    return true;
                }
                cur = p.parent();
            }
            false
        }
    }
}

fn simple_matches<E: Element>(s: &super::selector::SimpleSelector, elem: E) -> bool {
    if let Some(t) = &s.type_name {
        match elem.local_name() {
            Some(n) if n.eq_ignore_ascii_case(t) => {}
            _ => return false,
        }
    }
    if let Some(id) = &s.id {
        if elem.get_attr("id").as_deref() != Some(id.as_str()) {
            return false;
        }
    }
    if !s.classes.is_empty() {
        let elem_classes = elem.class_list();
        if !s
            .classes
            .iter()
            .all(|c| elem_classes.iter().any(|e| e == c))
        {
            return false;
        }
    }
    for a in &s.attrs {
        let ok = match a {
            AttrSelector::Exists(name) => elem.get_attr(name).is_some(),
            AttrSelector::Equals(name, val) => elem.get_attr(name).as_deref() == Some(val.as_str()),
        };
        if !ok {
            return false;
        }
    }
    for n in &s.not {
        if simple_matches(n, elem) {
            return false;
        }
    }
    true
}

/// Port of `css_selectors.Select`: a small per-document index (just the
/// element list, in this scoped implementation -- Python builds
/// class/id/tag indices for speed, which this port does not need since
/// `has_matches` here is a linear scan; see the `css` module docs).
pub struct Select<E: Element> {
    elements: Vec<E>,
}

impl<E: Element> Select<E> {
    pub fn new(elements: Vec<E>) -> Self {
        Self { elements }
    }

    /// Port of `select.has_matches(selector_text)`: does any element in
    /// this document match any selector in `selectors`?
    pub fn has_matches(&self, selectors: &SelectorList) -> bool {
        self.elements.iter().any(|&e| selectors.matches(e))
    }

    /// Port of `tuple(select(selector_text))`: every element in this
    /// document matching any selector in `selectors`, in document order.
    pub fn matching(&self, selectors: &SelectorList) -> Vec<E> {
        self.elements
            .iter()
            .copied()
            .filter(|&e| selectors.matches(e))
            .collect()
    }
}

impl<'a> Select<DomElement<'a>> {
    pub fn for_dom(dom: &'a Dom) -> Self {
        Self::new(dom_elements(dom))
    }
}

impl<'a> Select<XmlElement<'a>> {
    pub fn for_xml(xml: &'a Xml) -> Self {
        Self::new(xml_elements(xml))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::selector::parse_selector_list;

    #[test]
    fn matches_type_class_and_descendant_combinator() {
        let dom =
            Dom::parse("<html><body><div class=\"outer\"><p class=\"a\">x</p></div></body></html>");
        let p = dom.find_first_tag_global("p").unwrap();
        let elem = DomElement { dom: &dom, id: p };
        let sel = parse_selector_list("div.outer p.a").unwrap();
        assert!(sel.matches(elem));
        let sel2 = parse_selector_list("div.other p").unwrap();
        assert!(!sel2.matches(elem));
    }

    #[test]
    fn matches_child_combinator_but_not_grandchild() {
        let dom = Dom::parse("<html><body><div><span><p>x</p></span></div></body></html>");
        let p = dom.find_first_tag_global("p").unwrap();
        let elem = DomElement { dom: &dom, id: p };
        assert!(!parse_selector_list("div > p").unwrap().matches(elem));
        assert!(parse_selector_list("span > p").unwrap().matches(elem));
        assert!(parse_selector_list("div p").unwrap().matches(elem));
    }

    #[test]
    fn matches_id_and_attribute_selectors() {
        let dom = Dom::parse("<html><body><a href=\"x\" id=\"main\">x</a></body></html>");
        let a = dom.find_first_tag_global("a").unwrap();
        let elem = DomElement { dom: &dom, id: a };
        assert!(parse_selector_list("#main").unwrap().matches(elem));
        assert!(parse_selector_list("[href=x]").unwrap().matches(elem));
        assert!(parse_selector_list("[href]").unwrap().matches(elem));
        assert!(!parse_selector_list("[href=y]").unwrap().matches(elem));
    }

    #[test]
    fn not_selector_excludes_matches() {
        let dom = Dom::parse("<html><body><p class=\"a\">x</p><p class=\"b\">y</p></body></html>");
        let ps = dom.find_all_tag_global("p");
        let sel = parse_selector_list("p:not(.a)").unwrap();
        let a_elem = DomElement {
            dom: &dom,
            id: ps[0],
        };
        let b_elem = DomElement {
            dom: &dom,
            id: ps[1],
        };
        assert!(!sel.matches(a_elem));
        assert!(sel.matches(b_elem));
    }

    #[test]
    fn select_has_matches_scans_the_whole_document() {
        let dom = Dom::parse("<html><body><div><p class=\"x\">hi</p></div></body></html>");
        let select = Select::for_dom(&dom);
        assert!(select.has_matches(&parse_selector_list(".x").unwrap()));
        assert!(!select.has_matches(&parse_selector_list(".missing").unwrap()));
    }

    #[test]
    fn xml_element_matches_by_local_name_and_attr() {
        let xml = Xml::parse(
            r#"<?xml version="1.0"?><package><manifest><item id="c1" href="a.html"/></manifest></package>"#,
        )
        .unwrap();
        let select = Select::for_xml(&xml);
        assert!(select.has_matches(&parse_selector_list("item[href]").unwrap()));
        assert!(!select.has_matches(&parse_selector_list("item[href=missing]").unwrap()));
    }
}
