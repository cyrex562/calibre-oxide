use crate::dom::{Dom, NodeId};
use crate::oeb::constants::*;

/// Extract the local name from a Clark-notation string (e.g., `{ns}tag` -> `tag`).
pub fn barename(name: &str) -> &str {
    if let Some(pos) = name.rfind('}') {
        &name[pos + 1..]
    } else {
        name
    }
}

pub fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Extract the namespace URI from a Clark-notation string.
pub fn namespace(name: &str) -> &str {
    if name.starts_with('{') {
        if let Some(pos) = name.find('}') {
            return &name[1..pos];
        }
    }
    ""
}

/// Helper to construct Clark notation for XHTML namespace
#[allow(non_snake_case)]
pub fn XHTML(name: &str) -> String {
    format!("{{{}}}{}", XHTML_NS, name)
}

/// Helper to construct Clark notation
pub fn qualified_name(ns: &str, name: &str) -> String {
    format!("{{{}}}{}", ns, name)
}

/// Port of `merge_multiple_html_heads_and_bodies`: if `root` has more
/// than one `<head>` or `<body>` descendant, replaces them all with a
/// single merged `<head>`/`<body>` pair (in that order) holding every
/// original head's/body's children concatenated in document order. Real
/// upstream's own `for child in root: root.remove(child)` unconditionally
/// clears every child of `root` first (not just heads/bodies) before
/// appending the merged pair -- preserved exactly, since any other
/// direct child of an `<html>` root is not a real-world case this needs
/// to protect.
pub fn merge_multiple_html_heads_and_bodies(dom: &mut Dom, root: NodeId) {
    let heads = dom.find_all_tag(root, "head");
    let bodies = dom.find_all_tag(root, "body");
    if heads.len() <= 1 && bodies.len() <= 1 {
        return;
    }
    let head = dom.new_element("head");
    let body = dom.new_element("body");
    for h in &heads {
        for child in dom.children(*h) {
            dom.append_child(head, child);
        }
    }
    for b in &bodies {
        for child in dom.children(*b) {
            dom.append_child(body, child);
        }
    }
    for child in dom.children(root) {
        dom.detach(child);
    }
    dom.append_child(root, head);
    dom.append_child(root, body);
}
