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
