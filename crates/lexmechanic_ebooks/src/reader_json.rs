//! Port of `old_src/src/calibre/srv/html_as_json.cpp` (issue #478,
//! part of #427's tracking epic) -- serializes an HTML document tree
//! into the in-browser reader's own compact JSON document format.
//!
//! # Shape (confirmed by reading the .cpp)
//!
//! An element becomes `{"n": tag_name, "x": text, "l": tail, "a":
//! [[attr_name, attr_value], ...], "c": [children...]}` -- every key
//! but `"n"` is omitted (not `null`) when absent, matching upstream's
//! own `StringOrNone` truthiness check. A comment (or any other
//! non-element lxml node upstream's serializer can see -- doctype,
//! processing instructions) becomes `{"s": "c", "x": text, "l":
//! tail}`. The whole document is `{"version": 1, "tree": <root
//! element>, "ns_map": [...]}`.
//!
//! `text`/`tail` mirror lxml's own element model, not a flat child
//! list: `text` is whatever comes right after the opening tag before
//! the first child element, `tail` is whatever comes right after an
//! element's closing tag before its next sibling. [`Dom`] represents
//! both as ordinary `Text` nodes interleaved with `Element`/`Comment`
//! children instead, so this module reconstructs the lxml view from
//! that: an element's own text is its first child if that's a `Text`
//! node ([`own_text`]); a node's tail is its immediate next sibling
//! if that's a `Text` node ([`Dom::next_sibling`] already exists for
//! exactly this -- see that method's own doc).
//!
//! # Not ported: XML namespaces
//!
//! Upstream's real input is an XHTML document parsed by lxml's XML
//! parser, so tag/attribute names can carry a namespace URI (Clark
//! notation, `{uri}local-name`) that gets assigned a compact integer
//! index and recorded in the trailing `ns_map` array (with one real
//! quirk reproduced faithfully were this namespace tracking ported: a
//! tag's own namespace index is only emitted when it's *not* index 0
//! -- the first-registered namespace, almost always the default XHTML
//! one -- while an attribute's namespace index is emitted whenever
//! it's non-negative; the asymmetry looks unintentional upstream but
//! is real, observable behavior). None of this is ported: [`Dom`] is
//! `html5ever`-backed HTML5 parsing, which has no concept of
//! arbitrary XML namespace URIs on tags/attributes the way lxml's XML
//! parser does -- every tag/attribute name it produces is already
//! bare. So every element here has no `"s"` (namespace index) key,
//! every attribute is always a 2-element `[name, value]` array (never
//! 3-element), and `ns_map` is always `[]`. A real port of this piece
//! would need namespace-URI-aware parsing added to `Dom` itself
//! first -- out of scope here.
//!
//! # Not ported: comment vs. other-node distinction
//!
//! Upstream distinguishes a real lxml `Comment` (`"s":"c"`) from any
//! other special node type it might encounter, e.g. a processing
//! instruction (`"s":"o"`). [`Dom`]'s own HTML5 parsing already
//! collapses doctypes and processing instructions into
//! `NodeKind::Comment(String::new())` (see `dom.rs`'s `convert`
//! function) -- that data loss happens before this module ever sees
//! the tree, so every [`NodeKind::Comment`] here is reported as
//! `"s":"c"`, not a pre-existing regression introduced by this port.

use serde_json::{json, Map, Value};

use crate::dom::{Dom, NodeId, NodeKind};

/// The immediate next sibling's text, if it's a `Text` node --
/// upstream's `element.tail`.
fn tail_of(dom: &Dom, id: NodeId) -> Option<String> {
    let sibling = dom.next_sibling(id)?;
    match &dom.node(sibling).kind {
        NodeKind::Text(t) => Some(t.clone()),
        _ => None,
    }
}

/// The first child's text, if it's a `Text` node -- upstream's
/// `element.text`.
fn own_text(dom: &Dom, id: NodeId) -> Option<String> {
    let first = *dom.node(id).children.first()?;
    match &dom.node(first).kind {
        NodeKind::Text(t) => Some(t.clone()),
        _ => None,
    }
}

fn element_json(dom: &Dom, id: NodeId, tag: &str) -> Value {
    let mut obj = Map::new();
    obj.insert("n".to_string(), Value::String(tag.to_string()));
    if let Some(text) = own_text(dom, id) {
        obj.insert("x".to_string(), Value::String(text));
    }
    if let Some(tail) = tail_of(dom, id) {
        obj.insert("l".to_string(), Value::String(tail));
    }
    let attrs = &dom.node(id).attrs;
    if !attrs.is_empty() {
        let arr: Vec<Value> = attrs.iter().map(|(k, v)| json!([k, v])).collect();
        obj.insert("a".to_string(), Value::Array(arr));
    }
    let children: Vec<NodeId> = dom.node(id).children.iter().copied().filter(|&c| matches!(dom.node(c).kind, NodeKind::Element(_) | NodeKind::Comment(_))).collect();
    if !children.is_empty() {
        let arr: Vec<Value> = children.iter().map(|&c| serialize_node(dom, c)).collect();
        obj.insert("c".to_string(), Value::Array(arr));
    }
    Value::Object(obj)
}

fn comment_json(dom: &Dom, id: NodeId, text: &str) -> Value {
    let mut obj = Map::new();
    obj.insert("s".to_string(), Value::String("c".to_string()));
    obj.insert("x".to_string(), Value::String(text.to_string()));
    if let Some(tail) = tail_of(dom, id) {
        obj.insert("l".to_string(), Value::String(tail));
    }
    Value::Object(obj)
}

/// Serializes the subtree rooted at `id` -- `id` should be an
/// `Element` or `Comment` node (matching upstream, always called with
/// a real root element); a `Document`/`Text` node serializes to
/// `null`, since upstream's own serializer is never called with one.
pub fn serialize_node(dom: &Dom, id: NodeId) -> Value {
    match &dom.node(id).kind {
        NodeKind::Element(tag) => element_json(dom, id, tag),
        NodeKind::Comment(text) => comment_json(dom, id, text),
        NodeKind::Document | NodeKind::Text(_) => Value::Null,
    }
}

/// Port of `Serializer::serialize`'s whole-document wrapper:
/// `{"version": 1, "tree": <serialize_node(root)>, "ns_map": []}` --
/// see this module's own doc for why `ns_map` is always empty here.
pub fn serialize_document(dom: &Dom, root: NodeId) -> Value {
    json!({
        "version": 1,
        "tree": serialize_node(dom, root),
        "ns_map": Value::Array(vec![]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root_element(dom: &Dom) -> NodeId {
        dom.find_first_tag_global("body").or_else(|| dom.find_first_tag_global("html")).unwrap()
    }

    #[test]
    fn a_bare_element_with_only_text_has_just_n_and_x() {
        let dom = Dom::parse("<html><body><p>hello</p></body></html>");
        let p = dom.find_first_tag_global("p").unwrap();
        let json = serialize_node(&dom, p);
        assert_eq!(json, serde_json::json!({"n": "p", "x": "hello"}));
    }

    #[test]
    fn an_element_with_no_text_omits_x_entirely() {
        let dom = Dom::parse("<html><body><br></body></html>");
        let br = dom.find_first_tag_global("br").unwrap();
        let json = serialize_node(&dom, br);
        assert_eq!(json, serde_json::json!({"n": "br"}));
        assert!(json.get("x").is_none());
    }

    #[test]
    fn a_following_sibling_text_node_becomes_the_tail() {
        let dom = Dom::parse("<html><body><div><p>hi</p>tail text</div></body></html>");
        let p = dom.find_first_tag_global("p").unwrap();
        let json = serialize_node(&dom, p);
        assert_eq!(json["l"], "tail text");
    }

    #[test]
    fn attributes_are_two_element_name_value_arrays() {
        let dom = Dom::parse(r#"<html><body><a href="url" class="c">text</a></body></html>"#);
        let a = dom.find_first_tag_global("a").unwrap();
        let json = serialize_node(&dom, a);
        assert_eq!(json["a"], serde_json::json!([["href", "url"], ["class", "c"]]));
    }

    #[test]
    fn an_element_with_no_attributes_omits_a_entirely() {
        let dom = Dom::parse("<html><body><p>hi</p></body></html>");
        let p = dom.find_first_tag_global("p").unwrap();
        let json = serialize_node(&dom, p);
        assert!(json.get("a").is_none());
    }

    #[test]
    fn nested_element_children_are_serialized_in_document_order() {
        let dom = Dom::parse("<html><body><div><p>a</p><p>b</p></div></body></html>");
        let div = dom.find_first_tag_global("div").unwrap();
        let json = serialize_node(&dom, div);
        assert_eq!(json["c"], serde_json::json!([{"n": "p", "x": "a"}, {"n": "p", "x": "b"}]));
    }

    #[test]
    fn text_only_nodes_never_appear_in_the_children_array() {
        let dom = Dom::parse("<html><body><div>before<p>a</p>after</div></body></html>");
        let div = dom.find_first_tag_global("div").unwrap();
        let json = serialize_node(&dom, div);
        assert_eq!(json["x"], "before");
        assert_eq!(json["c"].as_array().unwrap().len(), 1, "only the <p>, no bare text nodes");
        assert_eq!(json["c"][0]["l"], "after");
    }

    #[test]
    fn a_comment_serializes_with_type_c_and_its_own_text() {
        let dom = Dom::parse("<html><body><div><!-- a comment --></div></body></html>");
        let div = dom.find_first_tag_global("div").unwrap();
        let json = serialize_node(&dom, div);
        let comment = &json["c"][0];
        assert_eq!(comment["s"], "c");
        assert_eq!(comment["x"], " a comment ");
        assert!(comment.get("n").is_none(), "a comment node has no tag name");
    }

    #[test]
    fn a_comment_can_have_a_tail_too() {
        let dom = Dom::parse("<html><body><div><!--c-->after comment</div></body></html>");
        let div = dom.find_first_tag_global("div").unwrap();
        let json = serialize_node(&dom, div);
        assert_eq!(json["c"][0]["l"], "after comment");
    }

    #[test]
    fn serialize_document_wraps_with_version_and_an_empty_ns_map() {
        let dom = Dom::parse("<html><body><p>hi</p></body></html>");
        let body = root_element(&dom);
        let json = serialize_document(&dom, body);
        assert_eq!(json["version"], 1);
        assert_eq!(json["ns_map"], serde_json::json!([]));
        assert_eq!(json["tree"]["n"], "body");
    }

    #[test]
    fn deeply_nested_elements_round_trip_the_whole_subtree() {
        let dom = Dom::parse("<html><body><ul><li><a href=\"x\">one</a></li><li>two</li></ul></body></html>");
        let ul = dom.find_first_tag_global("ul").unwrap();
        let json = serialize_node(&dom, ul);
        assert_eq!(json["n"], "ul");
        let items = json["c"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["c"][0]["n"], "a");
        assert_eq!(items[0]["c"][0]["x"], "one");
        assert_eq!(items[1]["x"], "two");
    }
}
