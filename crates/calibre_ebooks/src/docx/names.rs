//! OOXML namespace tables and element-traversal helpers.
//!
//! Port of `old_src/src/calibre/ebooks/docx/names.py`.
//!
//! DOCX comes in two flavours of the same vocabulary: **transitional**
//! (what Word actually writes, `schemas.openxmlformats.org/...`) and
//! **strict** (ISO/IEC 29500 Strict, `purl.oclc.org/ooxml/...`). Which
//! one a file uses is decided by the package relationships and is
//! detected in [`crate::docx::container`]. Every namespace URI and
//! relationship type differs between them, so all traversal goes
//! through a [`DocxNamespace`] carrying the right table rather than
//! through hard-coded constants.
//!
//! The Python original leans on lxml's XPath, building expressions like
//! `./w:pPr` and `ancestor::w:p[1]` at every call site. Rather than
//! embedding an XPath engine, this port offers the four access patterns
//! those expressions actually use — [`children`], [`descendants`],
//! [`ancestor`], and [`get`] — over `roxmltree` nodes.
//!
//! [`children`]: DocxNamespace::children
//! [`descendants`]: DocxNamespace::descendants
//! [`ancestor`]: DocxNamespace::ancestor
//! [`get`]: DocxNamespace::get

use std::collections::{HashMap, HashSet};

use calibre_utils::filenames::ascii_text;
use roxmltree::Node;

/// The `<a:ext uri>` marking an SVG alternative to a raster blip.
pub const SVG_BLIP_URI: &str = "{96DAC541-7B7A-43D3-8B79-37D633B846F1}";
/// The `<a:ext uri>` marking "use the image's own DPI".
pub const USE_LOCAL_DPI_URI: &str = "{28A0092B-C50C-407E-A947-70E740481C1C}";

/// The XML namespace, identical in both flavours.
pub const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// Relationship types, transitional flavour, keyed as in the Python
/// `TRANSITIONAL_NAMES`.
const TRANSITIONAL_NAMES: &[(&str, &str)] = &[
    (
        "DOCUMENT",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument",
    ),
    (
        "DOCPROPS",
        "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties",
    ),
    (
        "APPPROPS",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties",
    ),
    (
        "STYLES",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles",
    ),
    (
        "NUMBERING",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering",
    ),
    (
        "FONTS",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/fontTable",
    ),
    (
        "EMBEDDED_FONT",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/font",
    ),
    (
        "IMAGES",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image",
    ),
    (
        "LINKS",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink",
    ),
    (
        "FOOTNOTES",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes",
    ),
    (
        "ENDNOTES",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/endnotes",
    ),
    (
        "THEMES",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme",
    ),
    (
        "SETTINGS",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings",
    ),
    (
        "WEB_SETTINGS",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/webSettings",
    ),
];

/// Namespace prefixes, transitional flavour, keyed as in the Python
/// `TRANSITIONAL_NAMESPACES`.
const TRANSITIONAL_NAMESPACES: &[(&str, &str)] = &[
    (
        "mo",
        "http://schemas.microsoft.com/office/mac/office/2008/main",
    ),
    ("o", "urn:schemas-microsoft-com:office:office"),
    (
        "ve",
        "http://schemas.openxmlformats.org/markup-compatibility/2006",
    ),
    (
        "mc",
        "http://schemas.openxmlformats.org/markup-compatibility/2006",
    ),
    // Text content
    (
        "w",
        "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
    ),
    ("w10", "urn:schemas-microsoft-com:office:word"),
    (
        "wne",
        "http://schemas.microsoft.com/office/word/2006/wordml",
    ),
    ("xml", XML_NS),
    // Drawing
    ("a", "http://schemas.openxmlformats.org/drawingml/2006/main"),
    (
        "a14",
        "http://schemas.microsoft.com/office/drawing/2010/main",
    ),
    (
        "m",
        "http://schemas.openxmlformats.org/officeDocument/2006/math",
    ),
    ("mv", "urn:schemas-microsoft-com:mac:vml"),
    (
        "pic",
        "http://schemas.openxmlformats.org/drawingml/2006/picture",
    ),
    ("v", "urn:schemas-microsoft-com:vml"),
    (
        "wp",
        "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
    ),
    // Properties (core and extended)
    (
        "cp",
        "http://schemas.openxmlformats.org/package/2006/metadata/core-properties",
    ),
    ("dc", "http://purl.org/dc/elements/1.1/"),
    (
        "ep",
        "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties",
    ),
    ("xsi", "http://www.w3.org/2001/XMLSchema-instance"),
    // Content types
    (
        "ct",
        "http://schemas.openxmlformats.org/package/2006/content-types",
    ),
    // Package relationships
    (
        "r",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    ),
    (
        "pr",
        "http://schemas.openxmlformats.org/package/2006/relationships",
    ),
    // Dublin Core document properties
    ("dcmitype", "http://purl.org/dc/dcmitype/"),
    ("dcterms", "http://purl.org/dc/terms/"),
    // SVG embeds
    (
        "asvg",
        "http://schemas.microsoft.com/office/drawing/2016/SVG/main",
    ),
];

/// The transitional → strict URI rewrites, applied in order. Same
/// substitutions the Python performs to derive `STRICT_NAMESPACES` and
/// `STRICT_NAMES` from their transitional counterparts.
const STRICT_REWRITES: &[(&str, &str)] = &[
    (
        "http://schemas.openxmlformats.org/officeDocument/2006",
        "http://purl.oclc.org/ooxml/officeDocument",
    ),
    (
        "http://schemas.openxmlformats.org/wordprocessingml/2006",
        "http://purl.oclc.org/ooxml/wordprocessingml",
    ),
    (
        "http://schemas.openxmlformats.org/drawingml/2006",
        "http://purl.oclc.org/ooxml/drawingml",
    ),
];

/// The relationship type identifying the main document part in a strict
/// package. Its presence is how [`crate::docx::container`] tells the two
/// flavours apart.
pub const STRICT_DOCUMENT_RELATIONSHIP: &str =
    "http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument";

fn to_strict(uri: &str) -> String {
    let mut out = uri.to_string();
    for (from, to) in STRICT_REWRITES {
        out = out.replace(from, to);
    }
    out
}

/// The local name of a fully qualified `{uri}local` tag.
///
/// Port of the Python `barename`.
pub fn barename(tag: &str) -> &str {
    match tag.rfind('}') {
        Some(i) => &tag[i + 1..],
        None => tag,
    }
}

/// A fully qualified XML name in the `xml` namespace, e.g.
/// `XML("space")` for `xml:space`.
///
/// Port of the Python `XML`.
pub fn xml_name(local: &str) -> String {
    format!("{{{XML_NS}}}{local}")
}

/// Derive an HTML anchor id from `name` that does not collide with
/// anything in `existing`.
///
/// Port of the Python `generate_anchor`. Non-alphanumerics are dropped
/// after transliteration to ASCII, and a numeric suffix disambiguates.
pub fn generate_anchor(name: &str, existing: &HashSet<String>) -> String {
    let cleaned: String = ascii_text(name)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    let base = format!("id_{}", cleaned.trim_start_matches('_'));
    if !existing.contains(&base) {
        return base;
    }
    let mut counter = 1usize;
    loop {
        let candidate = format!("{base}_{counter}");
        if !existing.contains(&candidate) {
            return candidate;
        }
        counter += 1;
    }
}

/// The namespace table for one DOCX package, plus the traversal helpers
/// that use it.
///
/// Port of the Python `DOCXNamespace`.
#[derive(Debug, Clone)]
pub struct DocxNamespace {
    /// True for the transitional (Word-authored) flavour.
    pub transitional: bool,
    namespaces: HashMap<&'static str, String>,
    names: HashMap<&'static str, String>,
}

impl Default for DocxNamespace {
    fn default() -> Self {
        Self::new(true)
    }
}

impl DocxNamespace {
    /// Build the table for the transitional or strict flavour.
    pub fn new(transitional: bool) -> Self {
        let map = |pairs: &[(&'static str, &str)]| -> HashMap<&'static str, String> {
            pairs
                .iter()
                .map(|(k, v)| {
                    let uri = if transitional {
                        (*v).to_string()
                    } else {
                        to_strict(v)
                    };
                    (*k, uri)
                })
                .collect()
        };
        Self {
            transitional,
            namespaces: map(TRANSITIONAL_NAMESPACES),
            names: map(TRANSITIONAL_NAMES),
        }
    }

    /// The URI bound to a prefix, e.g. `"w"`.
    pub fn namespace(&self, prefix: &str) -> Option<&str> {
        self.namespaces.get(prefix).map(String::as_str)
    }

    /// The relationship type for a key, e.g. `"DOCUMENT"`.
    pub fn name(&self, key: &str) -> Option<&str> {
        self.names.get(key).map(String::as_str)
    }

    /// Split `w:pPr` into its namespace URI and local name. An
    /// unprefixed name yields no URI, matching the Python `expand`,
    /// which returns a bare tag when there is no prefix.
    pub fn expand<'a>(&self, qname: &'a str) -> (Option<&str>, &'a str) {
        match qname.split_once(':') {
            Some((prefix, local)) if !local.is_empty() => (self.namespace(prefix), local),
            _ => (None, qname),
        }
    }

    /// Whether `node` is the element named by `qname`, e.g. `"w:p"`.
    ///
    /// Port of the Python `is_tag`.
    pub fn is_tag(&self, node: Node, qname: &str) -> bool {
        let (uri, local) = self.expand(qname);
        node.is_element() && node.tag_name().name() == local && node.tag_name().namespace() == uri
    }

    /// An attribute value by qualified name, e.g. `get(elem, "w:val")`.
    ///
    /// Port of the Python `get`.
    pub fn get<'a>(&self, node: Node<'a, '_>, qname: &str) -> Option<&'a str> {
        match self.expand(qname) {
            (Some(uri), local) => node.attribute((uri, local)),
            (None, local) => node.attribute(local),
        }
    }

    /// [`get`](Self::get), falling back to `default` when the attribute
    /// is absent.
    pub fn get_or<'a>(&self, node: Node<'a, '_>, qname: &str, default: &'a str) -> &'a str {
        self.get(node, qname).unwrap_or(default)
    }

    /// Direct element children matching any of `qnames`, in document
    /// order.
    ///
    /// Port of the Python `children`, and of the `./w:x` XPath idiom.
    pub fn children<'a, 'i>(&self, node: Node<'a, 'i>, qnames: &[&str]) -> Vec<Node<'a, 'i>> {
        node.children()
            .filter(|c| qnames.iter().any(|q| self.is_tag(*c, q)))
            .collect()
    }

    /// The first direct child matching `qname`.
    pub fn first_child<'a, 'i>(&self, node: Node<'a, 'i>, qname: &str) -> Option<Node<'a, 'i>> {
        node.children().find(|c| self.is_tag(*c, qname))
    }

    /// All descendants matching any of `qnames`, in document order,
    /// excluding `node` itself.
    ///
    /// Port of the Python `descendants`, and of the `descendant::w:x`
    /// and `//w:x` XPath idioms.
    pub fn descendants<'a, 'i>(&self, node: Node<'a, 'i>, qnames: &[&str]) -> Vec<Node<'a, 'i>> {
        node.descendants()
            .skip(1)
            .filter(|c| qnames.iter().any(|q| self.is_tag(*c, q)))
            .collect()
    }

    /// The nearest ancestor matching `qname`.
    ///
    /// Port of the Python `ancestor`, i.e. `ancestor::w:x[1]`.
    pub fn ancestor<'a, 'i>(&self, node: Node<'a, 'i>, qname: &str) -> Option<Node<'a, 'i>> {
        node.ancestors().skip(1).find(|a| self.is_tag(*a, qname))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roxmltree::Document;

    const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

    fn doc() -> Document<'static> {
        Document::parse(
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
                 <w:body>
                   <w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:t>hi</w:t></w:r></w:p>
                   <w:p><w:r><w:t>bye</w:t></w:r></w:p>
                 </w:body>
               </w:document>"#,
        )
        .expect("valid XML")
    }

    #[test]
    fn strict_flavour_rewrites_every_uri() {
        let t = DocxNamespace::new(true);
        let s = DocxNamespace::new(false);
        assert_eq!(t.namespace("w"), Some(W));
        assert_eq!(
            s.namespace("w"),
            Some("http://purl.oclc.org/ooxml/wordprocessingml/main")
        );
        assert_eq!(s.name("DOCUMENT"), Some(STRICT_DOCUMENT_RELATIONSHIP));
        // The XML namespace is not an OOXML one and must survive intact.
        assert_eq!(s.namespace("xml"), Some(XML_NS));
        // Nor are the Dublin Core and Microsoft-proprietary ones.
        assert_eq!(s.namespace("dc"), Some("http://purl.org/dc/elements/1.1/"));
        assert_eq!(
            s.namespace("o"),
            Some("urn:schemas-microsoft-com:office:office")
        );
    }

    #[test]
    fn expand_handles_unprefixed_names() {
        let ns = DocxNamespace::default();
        assert_eq!(ns.expand("w:val"), (Some(W), "val"));
        assert_eq!(ns.expand("Target"), (None, "Target"));
        // An unknown prefix has no URI but keeps its local name.
        assert_eq!(ns.expand("zz:thing"), (None, "thing"));
    }

    #[test]
    fn traversal_matches_the_xpath_idioms_it_replaces() {
        let ns = DocxNamespace::default();
        let doc = doc();
        let body = ns
            .descendants(doc.root_element(), &["w:body"])
            .into_iter()
            .next()
            .expect("body");

        let paras = ns.children(body, &["w:p"]);
        assert_eq!(paras.len(), 2);

        // ./w:pPr on the first paragraph, and w:val off its w:jc.
        let ppr = ns.first_child(paras[0], "w:pPr").expect("pPr");
        let jc = ns.first_child(ppr, "w:jc").expect("jc");
        assert_eq!(ns.get(jc, "w:val"), Some("center"));
        assert_eq!(ns.get(jc, "w:missing"), None);
        assert_eq!(ns.get_or(jc, "w:missing", "left"), "left");

        // descendant::w:t across the whole body.
        let texts: Vec<&str> = ns
            .descendants(body, &["w:t"])
            .iter()
            .filter_map(|n| n.text())
            .collect();
        assert_eq!(texts, vec!["hi", "bye"]);

        // ancestor::w:p[1] from a run.
        let run = ns.first_child(paras[1], "w:r").expect("r");
        let t = ns.first_child(run, "w:t").expect("t");
        assert_eq!(ns.ancestor(t, "w:p"), Some(paras[1]));
        assert_eq!(ns.ancestor(t, "w:tbl"), None);
    }

    #[test]
    fn children_of_several_names_stay_in_document_order() {
        let ns = DocxNamespace::default();
        let doc = doc();
        let p = ns.descendants(doc.root_element(), &["w:p"])[0];
        let kids = ns.children(p, &["w:r", "w:pPr"]);
        let names: Vec<&str> = kids.iter().map(|n| n.tag_name().name()).collect();
        assert_eq!(names, vec!["pPr", "r"]);
    }

    #[test]
    fn a_transitional_table_does_not_match_strict_markup() {
        // Guards the flavour detection: reading a strict file with the
        // transitional table must find nothing rather than silently
        // half-working.
        let strict = Document::parse(
            r#"<w:p xmlns:w="http://purl.oclc.org/ooxml/wordprocessingml/main">
                 <w:r/>
               </w:p>"#,
        )
        .unwrap();
        let transitional = DocxNamespace::new(true);
        assert!(transitional
            .children(strict.root_element(), &["w:r"])
            .is_empty());
        assert!(
            DocxNamespace::new(false)
                .children(strict.root_element(), &["w:r"])
                .len()
                == 1
        );
    }

    #[test]
    fn barename_strips_the_namespace() {
        assert_eq!(barename("{http://example.com}p"), "p");
        assert_eq!(barename("p"), "p");
        assert_eq!(xml_name("space"), format!("{{{XML_NS}}}space"));
    }

    #[test]
    fn anchors_are_unique_and_ascii() {
        let mut existing = HashSet::new();
        let a = generate_anchor("Chapter 1: Beginnings", &existing);
        assert_eq!(a, "id_Chapter1Beginnings");
        existing.insert(a.clone());
        let b = generate_anchor("Chapter 1: Beginnings", &existing);
        assert_eq!(b, "id_Chapter1Beginnings_1");
        existing.insert(b);
        assert_eq!(
            generate_anchor("Chapter 1: Beginnings", &existing),
            "id_Chapter1Beginnings_2"
        );
    }

    #[test]
    fn anchor_of_an_empty_name_is_still_a_valid_id() {
        // An id must not start with a digit or be bare punctuation, so
        // the `id_` prefix always survives.
        let existing = HashSet::new();
        assert_eq!(generate_anchor("", &existing), "id_");
        assert_eq!(generate_anchor("...", &existing), "id_");
        assert_eq!(generate_anchor("_lead", &existing), "id_lead");
        assert_eq!(generate_anchor("42", &existing), "id_42");
    }
}
