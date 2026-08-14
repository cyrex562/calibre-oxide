//! Port of `Extract.search_page_img`/`Extract.filter_cover`, adapted from
//! odfpy's typed element API (`self.document.topnode.getElementsByType`,
//! `frm.getAttrNS`, `frm.parentNode`) to `roxmltree`'s read-only tree,
//! operating directly on `content.xml`'s pre-conversion text.
//!
//! `roxmltree` has no mutation API, so `filter_cover` -- which in Python
//! removes a DOM node in place -- instead uses `Node::range()` (the
//! matched paragraph's exact byte span in the original document) to
//! splice the paragraph's XML text out of `content.xml`, producing a new
//! `content.xml` string to convert instead of the original.
//!
//! **Gap**: `crate::metadata::odt::get_metadata` (already a real, tested
//! port from a prior issue) does not currently populate an
//! `odf_cover_frame` name / cover href the way Python's
//! `calibre.ebooks.metadata.odt.get_metadata` does (there's no such field
//! on `MetaInformation` at all), so [`crate::input::odt_input`] has
//! nothing to pass `filter_cover` today -- it's real, unit-tested code,
//! just not yet reachable from the input plugin's orchestration. Wiring
//! cover-frame detection into the metadata layer is out of scope for this
//! issue (that module was already ported and closed under a different
//! issue); the function is written so that whenever that gap is closed,
//! wiring it in here is a one-line call.

use crate::odt::namespaces::{DRAWNS, TEXTNS, XLINKNS};
use anyhow::{Context, Result};
use roxmltree::Document;

/// Port of `Extract.search_page_img`: `true` if any `draw:frame` is
/// anchored to the page (`text:anchor-type="page"`), meaning any pictures
/// on it will all end up before the first page of flowed content.
pub fn has_page_anchored_frame(content_doc: &Document) -> bool {
    content_doc.descendants().any(|n| {
        n.is_element()
            && n.tag_name().namespace() == Some(DRAWNS)
            && n.tag_name().name() == "frame"
            && n.attribute((TEXTNS, "anchor-type")) == Some("page")
    })
}

/// Port of `Extract.filter_cover`. If `content_xml` contains a
/// `draw:frame` named `cover_frame_name` whose sole child is a
/// `draw:image` with `xlink:href == cover_href`, and that frame is in
/// turn the sole child of its enclosing `text:p`, removes that whole
/// paragraph from the document and returns the resulting XML. Returns
/// `Ok(None)` if no such exact shape is found (matches the original
/// silently doing nothing rather than raising).
pub fn filter_cover(
    content_xml: &str,
    cover_frame_name: &str,
    cover_href: &str,
) -> Result<Option<String>> {
    let doc = Document::parse(content_xml).context("parsing content.xml")?;
    for frame in doc.descendants().filter(|n| {
        n.is_element() && n.tag_name().namespace() == Some(DRAWNS) && n.tag_name().name() == "frame"
    }) {
        if frame.attribute((DRAWNS, "name")) != Some(cover_frame_name) {
            continue;
        }
        let element_children: Vec<_> = frame.children().filter(|c| c.is_element()).collect();
        if element_children.len() != 1 {
            continue;
        }
        let image = element_children[0];
        let is_image =
            image.tag_name().namespace() == Some(DRAWNS) && image.tag_name().name() == "image";
        if !is_image || image.attribute((XLINKNS, "href")) != Some(cover_href) {
            continue;
        }
        let Some(para) = frame.parent() else {
            continue;
        };
        let is_text_p = para.is_element()
            && para.tag_name().namespace() == Some(TEXTNS)
            && para.tag_name().name() == "p";
        if !is_text_p || para.children().filter(|c| c.is_element()).count() != 1 {
            continue;
        }
        let range = para.range();
        let mut out = String::with_capacity(content_xml.len());
        out.push_str(&content_xml[..range.start]);
        out.push_str(&content_xml[range.end..]);
        return Ok(Some(out));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NS_HEADER: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:xlink="http://www.w3.org/1999/xlink""#;

    #[test]
    fn detects_page_anchored_frame() {
        let xml = format!(
            r#"<office:document-content {NS_HEADER}><office:body><office:text>
            <text:p><draw:frame draw:name="F1" text:anchor-type="page"><draw:image xlink:href="Pictures/a.png"/></draw:frame></text:p>
            </office:text></office:body></office:document-content>"#
        );
        let doc = Document::parse(&xml).unwrap();
        assert!(has_page_anchored_frame(&doc));
    }

    #[test]
    fn no_page_anchored_frame() {
        let xml = format!(
            r#"<office:document-content {NS_HEADER}><office:body><office:text>
            <text:p><draw:frame draw:name="F1" text:anchor-type="paragraph"><draw:image xlink:href="Pictures/a.png"/></draw:frame></text:p>
            </office:text></office:body></office:document-content>"#
        );
        let doc = Document::parse(&xml).unwrap();
        assert!(!has_page_anchored_frame(&doc));
    }

    #[test]
    fn removes_matching_cover_paragraph() {
        let xml = format!(
            r#"<office:document-content {NS_HEADER}><office:body><office:text>
            <text:p><draw:frame draw:name="CoverFrame"><draw:image xlink:href="Pictures/cover.png"/></draw:frame></text:p>
            <text:p>Chapter one</text:p>
            </office:text></office:body></office:document-content>"#
        );
        let result = filter_cover(&xml, "CoverFrame", "Pictures/cover.png")
            .unwrap()
            .expect("cover paragraph removed");
        assert!(!result.contains("CoverFrame"));
        assert!(result.contains("Chapter one"));
        // Must still be well-formed XML after the splice.
        Document::parse(&result).unwrap();
    }

    #[test]
    fn leaves_document_alone_when_frame_has_siblings() {
        let xml = format!(
            r#"<office:document-content {NS_HEADER}><office:body><office:text>
            <text:p><draw:frame draw:name="CoverFrame"><draw:image xlink:href="Pictures/cover.png"/></draw:frame><text:span>caption</text:span></text:p>
            </office:text></office:body></office:document-content>"#
        );
        let result = filter_cover(&xml, "CoverFrame", "Pictures/cover.png").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn leaves_document_alone_when_href_mismatches() {
        let xml = format!(
            r#"<office:document-content {NS_HEADER}><office:body><office:text>
            <text:p><draw:frame draw:name="CoverFrame"><draw:image xlink:href="Pictures/other.png"/></draw:frame></text:p>
            </office:text></office:body></office:document-content>"#
        );
        let result = filter_cover(&xml, "CoverFrame", "Pictures/cover.png").unwrap();
        assert!(result.is_none());
    }
}
