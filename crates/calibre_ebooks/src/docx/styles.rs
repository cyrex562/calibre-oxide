//! Page properties and named `<w:style>` entries.
//!
//! Partial port of `old_src/src/calibre/ebooks/docx/styles.py` --
//! [`PageProperties`] and [`Style`] (one `<w:style>` entry) only. The
//! `Styles` collection (the paragraph/run cascade orchestrator that
//! resolves `docDefaults -> named style -> direct formatting`) is
//! deferred: it needs `Tables::para_style`/`run_style`, and `Tables`
//! itself needs a mutable tree (see `super::tables`'s module docs and
//! issue #130).

use roxmltree::Node;

use super::block_styles::{twips, ParagraphStyle};
use super::char_styles::RunStyle;
use super::names::DocxNamespace;
use super::tables::TableStyle;

/// Page size/margins, read from `w:sectPr` elements. Defaults to A4
/// with 1in margins, Word's own defaults when nothing is specified.
///
/// Port of the Python `PageProperties`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageProperties {
    pub width: f64,
    pub height: f64,
    pub margin_left: f64,
    pub margin_right: f64,
}

impl Default for PageProperties {
    fn default() -> Self {
        Self {
            width: 595.28,
            height: 841.89,
            margin_left: 72.0,
            margin_right: 72.0,
        }
    }
}

impl PageProperties {
    pub fn new() -> Self {
        Self::default()
    }

    /// Port of `PageProperties(namespace, elems)`.
    pub fn from_sect_prs(elems: &[Node], ns: &DocxNamespace) -> Self {
        let mut p = Self::new();
        for &sect_pr in elems {
            for pg_sz in ns.children(sect_pr, &["w:pgSz"]) {
                if let Some(v) = twips(ns.get(pg_sz, "w:w"), 0.05) {
                    p.width = v;
                }
                if let Some(v) = twips(ns.get(pg_sz, "w:h"), 0.05) {
                    p.height = v;
                }
            }
            for pg_mar in ns.children(sect_pr, &["w:pgMar"]) {
                if let Some(v) = twips(ns.get(pg_mar, "w:left"), 0.05) {
                    p.margin_left = v;
                }
                if let Some(v) = twips(ns.get(pg_mar, "w:right"), 0.05) {
                    p.margin_right = v;
                }
            }
        }
        p
    }
}

/// The last of `elem`'s direct children matching `qname` that also
/// carries a `w:val` attribute -- the `./w:x[@w:val]` XPath idiom used
/// throughout `styles.py`, taking the *last* match (Python iterates
/// and keeps reassigning).
fn last_child_with_val<'a, 'i>(
    elem: Node<'a, 'i>,
    ns: &DocxNamespace,
    qname: &str,
) -> Option<Node<'a, 'i>> {
    ns.children(elem, &[qname])
        .into_iter()
        .filter(|n| ns.get(*n, "w:val").is_some())
        .last()
}

/// The first such child, for the one Python spot that takes `[0]`
/// instead of the last (`based_on`).
fn first_child_with_val<'a, 'i>(
    elem: Node<'a, 'i>,
    ns: &DocxNamespace,
    qname: &str,
) -> Option<Node<'a, 'i>> {
    ns.children(elem, &[qname])
        .into_iter()
        .find(|n| ns.get(*n, "w:val").is_some())
}

/// One `<w:style>` entry -- a named paragraph, character, table or
/// numbering style, and (via [`Style::resolve_based_on`]) its
/// `w:basedOn` inheritance chain fully resolved.
///
/// Port of the Python `Style`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Style {
    pub resolved: bool,
    pub style_id: Option<String>,
    pub style_type: Option<String>,
    pub name: Option<String>,
    pub based_on: Option<String>,
    pub is_default: bool,
    pub paragraph_style: Option<ParagraphStyle>,
    pub character_style: Option<RunStyle>,
    pub table_style: Option<TableStyle>,
    /// Only meaningful when `style_type` is `"numbering"` or
    /// `"paragraph"`; `None` for every other style type (mirroring
    /// the Python attribute simply not existing on those instances).
    pub numbering_style_link: Option<String>,
}

impl Style {
    /// Port of `Style(namespace, elem)`.
    pub fn from_elem(elem: Node, ns: &DocxNamespace) -> Self {
        let mut s = Self::default();
        s.style_id = ns.get(elem, "w:styleId").map(str::to_string);
        s.style_type = ns.get(elem, "w:type").map(str::to_string);
        s.name = last_child_with_val(elem, ns, "w:name")
            .and_then(|n| ns.get(n, "w:val"))
            .map(str::to_string);
        s.based_on = first_child_with_val(elem, ns, "w:basedOn")
            .and_then(|n| ns.get(n, "w:val"))
            .map(str::to_string);
        if s.style_type.as_deref() == Some("numbering") {
            s.based_on = None;
        }
        s.is_default = matches!(
            ns.get(elem, "w:default"),
            Some("1") | Some("on") | Some("true")
        );

        if matches!(
            s.style_type.as_deref(),
            Some("paragraph" | "character" | "table")
        ) {
            if s.style_type.as_deref() == Some("table") {
                for tblpr in ns.children(elem, &["w:tblPr"]) {
                    let ts = TableStyle::from_tblpr(tblpr, ns);
                    match &mut s.table_style {
                        None => s.table_style = Some(ts),
                        Some(existing) => existing.update(&ts),
                    }
                }
            }
            if matches!(s.style_type.as_deref(), Some("paragraph" | "table")) {
                for ppr in ns.children(elem, &["w:pPr"]) {
                    let ps = ParagraphStyle::from_ppr(ppr, ns);
                    match &mut s.paragraph_style {
                        None => s.paragraph_style = Some(ps),
                        Some(existing) => existing.update(&ps),
                    }
                }
            }
            for rpr in ns.children(elem, &["w:rPr"]) {
                let rs = RunStyle::from_rpr(rpr, ns);
                match &mut s.character_style {
                    None => s.character_style = Some(rs),
                    Some(existing) => existing.update(&rs),
                }
            }
        }

        if matches!(s.style_type.as_deref(), Some("numbering" | "paragraph")) {
            let mut link = None;
            for ppr in ns.children(elem, &["w:pPr"]) {
                for num_pr in ns.children(ppr, &["w:numPr"]) {
                    if let Some(num_id) = last_child_with_val(num_pr, ns, "w:numId") {
                        link = ns.get(num_id, "w:val").map(str::to_string);
                    }
                }
            }
            s.numbering_style_link = link;
        }

        s
    }

    /// Fills every unset (`None`) sub-style from `parent`'s.
    ///
    /// Port of the Python `Style.resolve_based_on`.
    pub fn resolve_based_on(&mut self, parent: &Style) {
        if let Some(parent_table) = &parent.table_style {
            let ts = self.table_style.get_or_insert_with(TableStyle::new);
            ts.resolve_based_on(parent_table);
        }
        if let Some(parent_para) = &parent.paragraph_style {
            let ps = self.paragraph_style.get_or_insert_with(ParagraphStyle::new);
            ps.resolve_based_on(parent_para);
        }
        if let Some(parent_char) = &parent.character_style {
            let rs = self.character_style.get_or_insert_with(RunStyle::new);
            rs.resolve_based_on(parent_char);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roxmltree::Document;

    const DOC_OPEN: &str =
        r#"<w:style xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#;

    fn style_of(style_type: &str, body: &str) -> Style {
        let xml: &'static str = Box::leak(
            format!(r#"{DOC_OPEN}<w:style w:type="{style_type}" w:styleId="S1">{body}</w:style></w:style>"#)
                .into_boxed_str(),
        );
        let doc = Document::parse(xml).expect("valid XML");
        let ns = DocxNamespace::default();
        let inner = ns.first_child(doc.root_element(), "w:style").unwrap();
        Style::from_elem(inner, &ns)
    }

    #[test]
    fn page_properties_default_to_a4() {
        let p = PageProperties::new();
        assert_eq!(p.width, 595.28);
        assert_eq!(p.margin_left, 72.0);
    }

    #[test]
    fn page_properties_reads_size_and_margins() {
        let (doc, ns) = {
            let xml: &'static str = Box::leak(
                r#"<w:sectPr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
                    <w:pgSz w:w="12240" w:h="15840"/>
                    <w:pgMar w:left="1440" w:right="1440"/>
                </w:sectPr>"#
                    .to_string()
                    .into_boxed_str(),
            );
            (
                Document::parse(xml).expect("valid XML"),
                DocxNamespace::default(),
            )
        };
        let p = PageProperties::from_sect_prs(&[doc.root_element()], &ns);
        assert_eq!(p.width, 612.0);
        assert_eq!(p.height, 792.0);
        assert_eq!(p.margin_left, 72.0);
        assert_eq!(p.margin_right, 72.0);
    }

    #[test]
    fn style_reads_name_and_based_on() {
        let s = style_of(
            "paragraph",
            r#"<w:name w:val="Heading 1"/><w:basedOn w:val="Normal"/>"#,
        );
        assert_eq!(s.name.as_deref(), Some("Heading 1"));
        assert_eq!(s.based_on.as_deref(), Some("Normal"));
        assert_eq!(s.style_id.as_deref(), Some("S1"));
    }

    #[test]
    fn numbering_style_never_has_a_based_on() {
        let s = style_of("numbering", r#"<w:basedOn w:val="Whatever"/>"#);
        assert_eq!(s.based_on, None);
    }

    #[test]
    fn table_style_type_builds_a_table_style_and_paragraph_style() {
        let s = style_of(
            "table",
            r#"<w:tblPr><w:tblW w:w="5000" w:type="pct"/></w:tblPr><w:pPr><w:jc w:val="center"/></w:pPr>"#,
        );
        assert!(s.table_style.is_some());
        assert_eq!(s.table_style.unwrap().width.as_deref(), Some("100%"));
        assert!(s.paragraph_style.is_some());
    }

    #[test]
    fn paragraph_style_numbering_link_reads_num_id() {
        let s = style_of(
            "paragraph",
            r#"<w:pPr><w:numPr><w:numId w:val="3"/></w:numPr></w:pPr>"#,
        );
        assert_eq!(s.numbering_style_link.as_deref(), Some("3"));
    }

    #[test]
    fn character_style_never_reads_a_numbering_link() {
        let s = style_of("character", r#"<w:rPr><w:b/></w:rPr>"#);
        assert_eq!(s.numbering_style_link, None);
    }

    #[test]
    fn resolve_based_on_creates_missing_sub_styles_from_the_parent() {
        let mut parent = Style::default();
        parent.table_style = Some({
            let mut ts = TableStyle::new();
            ts.width = Some("10pt".to_string());
            ts
        });
        let mut child = Style::default();
        child.resolve_based_on(&parent);
        assert_eq!(
            child.table_style.unwrap().width.as_deref(),
            Some("10pt"),
            "child had no table_style of its own, so it inherits the parent's wholesale"
        );
    }
}
