//! ODF XML namespace URIs and the short prefixes `odf2xhtml.py` uses when
//! turning `style:style` names into CSS class names.
//!
//! Port of the handful of constants from `old_src/src/odf/namespaces.py`
//! that the scoped-down ODT-\>XHTML converter (see
//! [`crate::odt::convert`]) actually needs. The full `odf` package (34
//! files, ~17,400 lines) is tracked separately under `docs/modules_to_port.md`'s
//! `## src/odf` section and is *not* ported here -- these are just the
//! namespace string constants, copied because a from-scratch converter
//! still has to recognize the same element/attribute qualified names the
//! original SAX handler dispatched on.

pub const OFFICENS: &str = "urn:oasis:names:tc:opendocument:xmlns:office:1.0";
pub const STYLENS: &str = "urn:oasis:names:tc:opendocument:xmlns:style:1.0";
pub const TEXTNS: &str = "urn:oasis:names:tc:opendocument:xmlns:text:1.0";
pub const TABLENS: &str = "urn:oasis:names:tc:opendocument:xmlns:table:1.0";
pub const DRAWNS: &str = "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0";
pub const FONS: &str = "urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0";
pub const SVGNS: &str = "urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0";
pub const XLINKNS: &str = "http://www.w3.org/1999/xlink";
pub const NUMBERNS: &str = "urn:oasis:names:tc:opendocument:xmlns:datastyle:1.0";

/// `odf2xhtml.ODF2XHTML.familymap`: ODF `style:family` value -> the HTML
/// element family it conceptually maps to. Only used here to decide the
/// short class-name prefix below; we don't otherwise need the HTML family
/// name the way the original streaming converter does.
pub fn family_short_prefix(family: &str) -> &'static str {
    // Port of `ODF2XHTML._familyshort`.
    match family {
        "drawing-page" => "DP",
        "paragraph" => "P",
        "presentation" => "PR",
        "text" => "S",
        "section" => "D",
        "table" => "T",
        "table-cell" => "TD",
        "table-column" => "TC",
        "table-row" => "TR",
        "graphic" => "G",
        _ => "X",
    }
}

/// A handful of well-known style names that get rendered as a specific
/// semantic HTML tag instead of a generic `<p class="…">`/`<span
/// class="…">`, matching `odf2xhtml.special_styles`. Only the entries
/// relevant to text documents (not presentation/spreadsheet-only styles)
/// are kept.
pub fn special_tag_for_class(class_name: &str) -> Option<&'static str> {
    match class_name {
        "S-Emphasis" => Some("em"),
        "S-Citation" => Some("cite"),
        "S-Strong_20_Emphasis" => Some("strong"),
        "S-Variable" => Some("var"),
        "S-Definition" => Some("dfn"),
        "S-Teletype" => Some("tt"),
        "P-Heading_20_1" => Some("h1"),
        "P-Heading_20_2" => Some("h2"),
        "P-Heading_20_3" => Some("h3"),
        "P-Heading_20_4" => Some("h4"),
        "P-Heading_20_5" => Some("h5"),
        "P-Heading_20_6" => Some("h6"),
        "P-Addressee" => Some("address"),
        "P-Preformatted_20_Text" => Some("pre"),
        _ => None,
    }
}

/// Sanitizes an ODF style name for use as a CSS class name / HTML class
/// token, matching the `name.replace('.', '_')` calls scattered throughout
/// `odf2xhtml.py`.
pub fn sanitize_style_name(name: &str) -> String {
    name.replace('.', "_")
}

/// Builds the CSS class name `odf2xhtml.py` would generate for a
/// `style:style` with the given `family` and `name`, e.g. `family =
/// "paragraph", name = "Heading.1"` -\> `"P-Heading_1"`.
pub fn class_name_for(family: &str, name: &str) -> String {
    format!(
        "{}-{}",
        family_short_prefix(family),
        sanitize_style_name(name)
    )
}
