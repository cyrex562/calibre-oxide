//! XML escaping.
//!
//! Port of `prepare_string_for_xml` from `old_src/src/calibre/__init__.py`,
//! which calibre keeps at its package root because everything that
//! writes markup needs it.

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
