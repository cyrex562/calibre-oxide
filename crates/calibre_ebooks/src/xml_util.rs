//! XML escaping, and a `roxmltree`-based pretty-printer.
//!
//! [`prepare_string_for_xml`] is a port of `prepare_string_for_xml`
//! from `old_src/src/calibre/__init__.py`, which calibre keeps at its
//! package root because everything that writes markup needs it.
//!
//! [`pretty_print`] was originally written for `docx::dump` (a
//! debugging aid that unzips a DOCX package and re-indents every part,
//! since Word writes them as one enormous line) and is promoted here
//! -- unchanged -- so other callers with the same "reformat an already-
//! parsed `roxmltree::Document` with one element per line" need (e.g.
//! `fb2::fb2ml`'s `--pretty-print` option, issue #145) don't have to
//! reach into a sibling format's module for it. `docx::dump` re-exports
//! it under its original name so that module reads as it did.

use crate::html_entities::xml_replace_entities;

/// Resolve entity references, then escape the XML metacharacters.
///
/// `attribute` additionally escapes the quote characters, for values
/// going into an attribute rather than into element content.
pub fn prepare_string_for_xml(raw: &str, attribute: bool) -> String {
    let mut out = xml_replace_entities(raw)
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    if attribute {
        out = out.replace('"', "&quot;").replace('\'', "&apos;");
    }
    out
}

/// Serialize a parsed document with one element per line.
///
/// Port of `docx::dump`'s original `pretty_print`.
pub fn pretty_print(doc: &roxmltree::Document) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    write_element(doc.root_element(), 0, true, &mut out);
    out
}

fn write_element(node: roxmltree::Node, depth: usize, is_root: bool, out: &mut String) {
    use roxmltree::NodeType;

    let indent = "  ".repeat(depth);
    out.push_str(&indent);
    out.push('<');
    let name = qualified_name(node);
    out.push_str(&name);

    if is_root {
        // Hoist every in-scope namespace onto the root, so descendants
        // can use bare prefixes.
        for ns in node.namespaces() {
            match ns.name() {
                Some(prefix) => {
                    out.push_str(&format!(" xmlns:{}=\"{}\"", prefix, escape_attr(ns.uri())))
                }
                None => out.push_str(&format!(" xmlns=\"{}\"", escape_attr(ns.uri()))),
            }
        }
    }
    for attr in node.attributes() {
        let name = match attr.namespace().and_then(|uri| node.lookup_prefix(uri)) {
            Some(prefix) => format!("{prefix}:{}", attr.name()),
            None => attr.name().to_string(),
        };
        out.push_str(&format!(" {name}=\"{}\"", escape_attr(attr.value())));
    }

    // Text content is written on the element's own line so that
    // significant whitespace inside `w:t` survives the round trip.
    let children: Vec<roxmltree::Node> = node
        .children()
        .filter(|c| match c.node_type() {
            NodeType::Element | NodeType::Comment => true,
            NodeType::Text => !c.text().unwrap_or("").trim().is_empty(),
            _ => false,
        })
        .collect();

    if children.is_empty() {
        out.push_str("/>\n");
        return;
    }
    let only_text = children.len() == 1 && children[0].node_type() == NodeType::Text;
    if only_text {
        out.push('>');
        out.push_str(&escape_text(children[0].text().unwrap_or("")));
        out.push_str(&format!("</{name}>\n"));
        return;
    }

    out.push_str(">\n");
    for child in children {
        match child.node_type() {
            NodeType::Element => write_element(child, depth + 1, false, out),
            NodeType::Text => {
                out.push_str(&"  ".repeat(depth + 1));
                out.push_str(&escape_text(child.text().unwrap_or("")));
                out.push('\n');
            }
            NodeType::Comment => {
                out.push_str(&"  ".repeat(depth + 1));
                out.push_str(&format!("<!--{}-->\n", child.text().unwrap_or("")));
            }
            _ => {}
        }
    }
    out.push_str(&indent);
    out.push_str(&format!("</{name}>\n"));
}

fn qualified_name(node: roxmltree::Node) -> String {
    match node
        .tag_name()
        .namespace()
        .and_then(|uri| node.lookup_prefix(uri))
        .filter(|p| !p.is_empty())
    {
        Some(prefix) => format!("{prefix}:{}", node.tag_name().name()),
        None => node.tag_name().name().to_string(),
    }
}

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attr(s: &str) -> String {
    escape_text(s).replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_the_metacharacters() {
        assert_eq!(prepare_string_for_xml("a < b", false), "a &lt; b");
        assert_eq!(prepare_string_for_xml("a > b", false), "a &gt; b");
        assert_eq!(prepare_string_for_xml("a & b", false), "a &amp; b");
    }

    #[test]
    fn resolves_entities_before_escaping() {
        // Otherwise `&amp;` would become `&amp;amp;`.
        assert_eq!(
            prepare_string_for_xml("Tom &amp; Jerry", false),
            "Tom &amp; Jerry"
        );
    }

    #[test]
    fn quotes_are_escaped_only_for_attributes() {
        assert_eq!(prepare_string_for_xml(r#"say "hi""#, false), r#"say "hi""#);
        assert_eq!(
            prepare_string_for_xml(r#"say "hi""#, true),
            "say &quot;hi&quot;"
        );
        assert_eq!(prepare_string_for_xml("it's", true), "it&apos;s");
    }
}
