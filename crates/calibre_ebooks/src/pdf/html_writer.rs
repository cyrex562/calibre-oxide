//! Port of `old_src/src/calibre/ebooks/pdf/html_writer.py` (1,238 lines).
//!
//! calibre's actual ebook -> PDF *output* writer: renders XHTML content
//! through a real Qt `QWebEnginePage` browser engine and drives Qt's
//! print-to-PDF pipeline (page layout, headers/footers, TOC bookmarks via
//! PDF outline, font merging against Chromium's print output).
//!
//! **Documented gap**: there is no clean local equivalent. Unlike issue
//! #41's SVG rasterization (where `resvg` was a genuine drop-in
//! replacement for Qt's SVG renderer), full CSS-aware HTML-to-PDF
//! rendering via a real browser engine has no comparable lightweight
//! pure-Rust crate - this stays a documented gap, matching this project's
//! prior precedent for `render_html.py` in `docs/modules_to_port.md`
//! ("deferred - Qt WebEngine; new viewer is a Tauri panel"). [`convert`]
//! carries the real signature with a `todo!()` body naming the dependency.
//!
//! What *is* separable and ported for real: [`Margins`]/[`dict_to_margins`]
//! (pure JSON-to-struct margin parsing, html_writer.py lines 507-509).
//!
//! What was considered and *not* separated out, with reasons:
//! - `update_metadata`/XMP packet generation (`metadata_to_xmp_packet`,
//!   html_writer.py line 28 import): that function lives in
//!   `calibre.ebooks.metadata.xmp` - a different Python module, tracked
//!   separately from this issue's six `pdf/` files. This crate's
//!   `crate::metadata::xmp` only has the *parse* direction
//!   (`metadata_from_xmp_packet`) so far, not the *generate* direction
//!   `html_writer.py` needs; adding packet generation is real work that
//!   belongs to whichever issue ports `metadata/xmp.py`, not this one.
//!   `update_metadata` itself also calls `set_metadata_implementation` on
//!   a live native PDF document object, so it's gated on the Qt rendering
//!   core regardless.
//! - The outline/ToC/bookmark machinery (`AnchorLocation`, `PDFOutlineRoot`,
//!   `annotate_toc`, `add_toc`, `get_anchor_locations`, `fix_links`,
//!   `get_page_number_display_map`, `add_pagenum_toc`, `add_header_footer`
//!   and the font-merging block `merge_fonts`/`merge_font_files`):
//!   `crates/calibre_ebooks/src/oeb/polish/toc.rs`'s `Toc`/`get_toc` (issue
//!   #163) *is* available and would back the ToC-tree-walking half of
//!   this faithfully, but every one of these functions is fundamentally
//!   about annotating or querying a **live, already-rendered** native PDF
//!   document object (`pdf_doc.extract_anchors()`, `pdf_doc.create_outline`,
//!   `pdf_doc.list_fonts`, `pdf_doc.alter_links`, ...) that only exists
//!   after Qt has rendered the page. There's no meaningful "port the pure
//!   part" cut here the way there was for `image_writer.py`'s page-size
//!   math: the geometry these functions manipulate (anchor pixel
//!   locations, PDF page numbers within the rendered output, embedded
//!   font glyph tables) is Qt's rendering output, not input the caller
//!   already has. They're left inside the documented rendering-core gap.

use serde_json::Value;

/// Port of `Margins` (`namedtuple('Margins', 'left top right bottom')`,
/// html_writer.py line 507). All fields optional exactly like the Python
/// original: `dict_to_margins` fills in whatever the source dict doesn't
/// have with a caller-supplied default (or `None`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Margins {
    pub left: Option<f64>,
    pub top: Option<f64>,
    pub right: Option<f64>,
    pub bottom: Option<f64>,
}

/// Port of `dict_to_margins` (html_writer.py lines 511-512): read
/// `left`/`top`/`right`/`bottom` numeric fields out of a JSON object
/// (as parsed from the `data-calibre-pdf-output-page-margins` HTML
/// attribute upstream), falling back to `default` for any field that's
/// missing or not a number.
pub fn dict_to_margins(val: &Value, default: Option<f64>) -> Margins {
    let get = |key: &str| -> Option<f64> { val.get(key).and_then(Value::as_f64).or(default) };
    Margins {
        left: get("left"),
        top: get("top"),
        right: get("right"),
        bottom: get("bottom"),
    }
}

/// Port of `convert` (html_writer.py line 1128 onward): render an OEB
/// book's XHTML spine to a PDF via a real browser engine. See the module
/// doc comment - this needs Qt's `QWebEnginePage`/`printToPdf` pipeline,
/// which has no local equivalent.
pub fn convert(_opf_path: &std::path::Path, _output_path: &std::path::Path) {
    todo!(
        "placeholder: needs a real CSS-aware HTML rendering engine (Qt QWebEnginePage in the \
         original) to drive print-to-PDF - no comparable lightweight pure-Rust crate exists; see \
         docs/modules_to_port.md's render_html.py precedent (deferred - Qt WebEngine; new viewer \
         is a Tauri panel)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dict_to_margins_reads_all_present_fields() {
        let v = json!({"left": 1.0, "top": 2.0, "right": 3.0, "bottom": 4.0});
        let m = dict_to_margins(&v, None);
        assert_eq!(
            m,
            Margins {
                left: Some(1.0),
                top: Some(2.0),
                right: Some(3.0),
                bottom: Some(4.0)
            }
        );
    }

    #[test]
    fn dict_to_margins_falls_back_to_default_for_missing_fields() {
        let v = json!({"left": 5.0});
        let m = dict_to_margins(&v, Some(0.0));
        assert_eq!(
            m,
            Margins {
                left: Some(5.0),
                top: Some(0.0),
                right: Some(0.0),
                bottom: Some(0.0)
            }
        );
    }

    #[test]
    fn dict_to_margins_default_none_leaves_missing_fields_none() {
        let v = json!({});
        let m = dict_to_margins(&v, None);
        assert_eq!(m, Margins::default());
    }

    #[test]
    #[should_panic(expected = "placeholder")]
    fn convert_is_a_documented_gap() {
        convert(
            std::path::Path::new("book.opf"),
            std::path::Path::new("out.pdf"),
        );
    }
}
