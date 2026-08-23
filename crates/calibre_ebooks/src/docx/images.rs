//! Port of `old_src/src/calibre/ebooks/docx/images.py` -- **the pure
//! geometry/CSS half only** (issue #289). Embedded-image extraction
//! (reading from the DOCX zip or a linked file, resizing, writing to
//! `dest_dir/images/`), the `Images` struct itself, and the
//! `w:drawing`/`w:pict` -> `<img>` markup generators (`pic_to_img`,
//! `drawing_to_html`, `pict_to_html`, `to_html`) are a separate,
//! larger follow-up -- this file covers exactly the parts that need no
//! filesystem/zip access and no `crate::dom` output: filename
//! sanitizing, EMU-to-point conversion, and the three functions that
//! compute an image's CSS (`get_image_properties`, `get_image_margins`,
//! `get_hpos`/`get_float_properties`).
//!
//! # Two reproduced upstream bugs
//!
//! - [`get_image_margins`] always returns an empty [`Css`]: Python's
//!   `emu_to_pt(val)` divides `val` -- a raw XML attribute *string*,
//!   e.g. `"91440"` -- by `12700` without ever calling `int()` first.
//!   `str / int` unconditionally raises `TypeError` in Python, caught
//!   by the surrounding `except (TypeError, ValueError): continue`.
//!   So the `distL`/`distT`/`distR`/`distB`-derived padding this
//!   function appears to compute is never actually produced, for any
//!   real document. Ported as the genuine no-op it is.
//! - [`get_hpos`]'s `wp:simplePos` fallback branch is dropped entirely:
//!   Python's `emu_to_pt(sp.get('x', None))` has the identical missing-
//!   `int()` bug, so that loop can never `return` -- and, having no
//!   side effects, is behaviorally identical to not existing at all.
//!   `get_hpos` here goes straight from the `wp:positionH` loop to the
//!   final `0.0` fallback, exactly matching real behavior.

use std::sync::OnceLock;

use calibre_utils::filenames::{ascii_filename, sanitize_file_name};
use regex::Regex;
use roxmltree::Node;

use super::block_styles::{format_g, pt, Css};
use super::names::DocxNamespace;
use super::styles::PageProperties;

fn non_filename_char_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[^0-9a-zA-Z.\-]").unwrap())
}

/// Port of `image_filename`.
pub fn image_filename(x: &str) -> String {
    let ascii = ascii_filename(x);
    let replaced = non_filename_char_re().replace_all(&ascii, "_");
    let trimmed = replaced.trim_start_matches('_').trim_start_matches('.');
    sanitize_file_name(trimmed)
}

/// Port of `emu_to_pt`: EMUs (English Metric Units, DrawingML's native
/// length unit) to points.
pub fn emu_to_pt(x: i64) -> f64 {
    x as f64 / 12700.0
}

/// Port of `pt_to_emu`.
pub fn pt_to_emu(x: f64) -> i64 {
    (x * 12700.0) as i64
}

/// Reads `./wp:extent` (width/height), `./wp:docPr` (alt text, title,
/// `hidden` -> `display: none`), and `./a:graphic//a:xfrm` (rotation,
/// horizontal/vertical flip -> a CSS `transform`) off `parent` (a
/// `wp:inline` or `wp:anchor`).
///
/// Port of `get_image_properties`.
pub fn get_image_properties<'a, 'i>(
    parent: Node<'a, 'i>,
    ns: &DocxNamespace,
) -> (Css, Option<String>, Option<String>) {
    let mut width = None;
    let mut height = None;
    for extent in ns.children(parent, &["wp:extent"]) {
        if let Some(cx) = ns.get(extent, "cx").and_then(|v| v.parse::<i64>().ok()) {
            width = Some(emu_to_pt(cx));
        }
        if let Some(cy) = ns.get(extent, "cy").and_then(|v| v.parse::<i64>().ok()) {
            height = Some(emu_to_pt(cy));
        }
    }
    let mut ans = Css::new();
    if let Some(w) = width {
        ans.insert("width".to_string(), pt(w));
    }
    if let Some(h) = height {
        ans.insert("height".to_string(), pt(h));
    }

    let mut alt = None;
    let mut title = None;
    for doc_pr in ns.children(parent, &["wp:docPr"]) {
        if let Some(d) = ns.get(doc_pr, "descr").filter(|s| !s.is_empty()) {
            alt = Some(d.to_string());
        }
        if let Some(t) = ns.get(doc_pr, "title").filter(|s| !s.is_empty()) {
            title = Some(t.to_string());
        }
        if matches!(
            ns.get(doc_pr, "hidden"),
            Some("true") | Some("on") | Some("1")
        ) {
            ans.insert("display".to_string(), "none".to_string());
        }
    }

    let mut transforms: Vec<String> = Vec::new();
    for graphic in ns.children(parent, &["a:graphic"]) {
        for xfrm in ns.descendants(graphic, &["a:xfrm"]) {
            if let Some(rot) = ns.get(xfrm, "rot").and_then(|v| v.parse::<i64>().ok()) {
                let rot = rot as f64 / 60000.0;
                if rot != 0.0 {
                    transforms.push(format!("rotate({}deg)", format_g(rot, 6)));
                }
            }
            if matches!(ns.get(xfrm, "flipH"), Some("1") | Some("true")) {
                transforms.push("scaleX(-1)".to_string());
            }
            if matches!(ns.get(xfrm, "flipV"), Some("1") | Some("true")) {
                transforms.push("scaleY(-1)".to_string());
            }
        }
    }
    if !transforms.is_empty() {
        ans.insert("transform".to_string(), transforms.join(" "));
    }

    (ans, alt, title)
}

/// Port of `get_image_margins`. **Always returns an empty [`Css`]** --
/// see the module docs for why this is a faithfully-reproduced
/// upstream bug, not an oversight.
pub fn get_image_margins(_elem: Node) -> Css {
    Css::new()
}

/// Reads `./wp:positionH` off `anchor`, resolving it to a horizontal
/// position fraction (`0.0` = fully left, `1.0` = fully right of the
/// page) via `relativeFrom`/`wp:align`/`wp:posOffset`, offset by
/// `width_frac` (the image's own half-width as a fraction of the page,
/// so a plain `posOffset` measures the image's *left edge* the same
/// way `relativeFrom="leftMargin"`/`"rightMargin"` do).
///
/// See the module docs for why the Python `wp:simplePos` fallback is
/// dropped here rather than reproduced.
///
/// Port of `get_hpos`.
pub fn get_hpos<'a, 'i>(
    anchor: Node<'a, 'i>,
    page_width: f64,
    ns: &DocxNamespace,
    width_frac: f64,
) -> f64 {
    for ph in ns.children(anchor, &["wp:positionH"]) {
        let rp = ns.get(ph, "relativeFrom");
        if rp == Some("leftMargin") {
            return width_frac;
        }
        if rp == Some("rightMargin") {
            return 1.0 + width_frac;
        }
        for align in ns.children(ph, &["wp:align"]) {
            let al = align.text().and_then(|t| match t {
                "left" => Some(0.0),
                "center" => Some(0.5),
                "right" => Some(1.0),
                _ => None,
            });
            if let Some(al) = al {
                return if rp == Some("page") {
                    al
                } else {
                    al + width_frac
                };
            }
        }
        for po in ns.children(ph, &["wp:posOffset"]) {
            if let Some(pos) = po.text().and_then(|t| t.trim().parse::<i64>().ok()) {
                return emu_to_pt(pos) / page_width + width_frac;
            }
        }
    }
    0.0
}

const WRAP_TAGS: &[&str] = &[
    "wrapNone",
    "wrapSquare",
    "wrapThrough",
    "wrapTight",
    "wrapTopAndBottom",
];
const NO_FLOAT_TAGS: &[&str] = &["wrapNone", "wrapTopAndBottom"];

/// Turns a floated/anchored image's `style` into real CSS positioning:
/// `display: block` (unless already set), the page-relative horizontal
/// position from [`get_hpos`] refined by the anchor's last
/// `wrapNone`/`wrapSquare`/`wrapThrough`/`wrapTight`/`wrapTopAndBottom`
/// child (a `float` for the "text wraps around it" styles, else
/// `margin: auto` centering/right-alignment), and whatever padding
/// [`get_image_margins`] produced (currently always none -- see the
/// module docs).
///
/// Port of `Images.get_float_properties`. A free function, not a
/// method, since (like every other function in this file) it only
/// ever touched `self.namespace`.
pub fn get_float_properties<'a, 'i>(
    anchor: Node<'a, 'i>,
    style: &mut Css,
    page: &PageProperties,
    ns: &DocxNamespace,
) {
    if !style.contains_key("display") {
        style.insert("display".to_string(), "block".to_string());
    }
    let mut padding = get_image_margins(anchor);
    let width_str = style
        .get("width")
        .cloned()
        .unwrap_or_else(|| "100pt".to_string());
    let width: f64 = width_str
        .get(..width_str.len().saturating_sub(2))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);

    let mut page_width = page.width - page.margin_left - page.margin_right;
    if page_width <= 0.0 {
        page_width = page.width;
    }

    let mut hpos = get_hpos(anchor, page_width, ns, width / (2.0 * page_width));

    let mut wrap_elem: Option<Node> = None;
    let mut dofloat = false;
    for child in anchor
        .children()
        .filter(|c| c.is_element())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let bt = child.tag_name().name();
        if WRAP_TAGS.contains(&bt) {
            dofloat = !NO_FLOAT_TAGS.contains(&bt);
            wrap_elem = Some(child);
            break;
        }
    }

    if let Some(wrap_elem) = wrap_elem {
        for (k, v) in get_image_margins(wrap_elem) {
            padding.insert(k, v);
        }
        let wt = ns.get(wrap_elem, "wrapText");
        hpos = match wt {
            Some("right") => 0.0,
            Some("left") => 1.0,
            _ => hpos,
        };
        if dofloat {
            style.insert(
                "float".to_string(),
                if hpos < 0.65 { "left" } else { "right" }.to_string(),
            );
        } else {
            let (ml, mr): (Option<&str>, Option<&str>) = if hpos < 0.34 {
                (None, None)
            } else if hpos > 0.65 {
                (Some("auto"), None)
            } else {
                (Some("auto"), Some("auto"))
            };
            if let Some(ml) = ml {
                style.insert("margin-left".to_string(), ml.to_string());
            }
            if let Some(mr) = mr {
                style.insert("margin-right".to_string(), mr.to_string());
            }
        }
    }

    for (k, v) in padding {
        style.insert(k, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roxmltree::Document;

    const DOC_OPEN: &str = r#"xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main""#;

    fn parse(body: &str) -> (Document<'static>, DocxNamespace) {
        let xml: &'static str =
            Box::leak(format!("<root {DOC_OPEN}>{body}</root>").into_boxed_str());
        (
            Document::parse(xml).expect("valid XML"),
            DocxNamespace::default(),
        )
    }

    mod image_filename_tests {
        use super::*;

        #[test]
        fn non_alnum_characters_become_underscores() {
            assert_eq!(image_filename("my pic!.png"), "my_pic_.png");
        }

        #[test]
        fn leading_underscores_and_dots_are_stripped() {
            assert_eq!(image_filename("__.hidden.png"), "hidden.png");
        }
    }

    #[test]
    fn emu_pt_round_trip() {
        assert_eq!(emu_to_pt(12700), 1.0);
        assert_eq!(pt_to_emu(1.0), 12700);
    }

    mod get_image_properties_tests {
        use super::*;

        #[test]
        fn extent_becomes_width_and_height_css() {
            let (doc, ns) = parse(r#"<wp:extent cx="914400" cy="457200"/>"#);
            let (css, _, _) = get_image_properties(doc.root_element(), &ns);
            assert_eq!(css.get("width").map(String::as_str), Some("72pt"));
            assert_eq!(css.get("height").map(String::as_str), Some("36pt"));
        }

        #[test]
        fn a_hidden_doc_pr_sets_display_none() {
            let (doc, ns) =
                parse(r#"<wp:docPr id="1" name="x" descr="a description" hidden="1"/>"#);
            let (css, alt, _) = get_image_properties(doc.root_element(), &ns);
            assert_eq!(css.get("display").map(String::as_str), Some("none"));
            assert_eq!(alt.as_deref(), Some("a description"));
        }

        #[test]
        fn a_title_is_read_from_doc_pr() {
            let (doc, ns) = parse(r#"<wp:docPr id="1" name="x" title="My Title"/>"#);
            let (_, _, title) = get_image_properties(doc.root_element(), &ns);
            assert_eq!(title.as_deref(), Some("My Title"));
        }

        #[test]
        fn rotation_and_flips_become_a_transform() {
            let (doc, ns) = parse(
                r#"<a:graphic><a:graphicData><a:xfrm rot="5400000" flipH="1" flipV="1"/></a:graphicData></a:graphic>"#,
            );
            let (css, _, _) = get_image_properties(doc.root_element(), &ns);
            assert_eq!(
                css.get("transform").map(String::as_str),
                Some("rotate(90deg) scaleX(-1) scaleY(-1)")
            );
        }

        #[test]
        fn no_extent_or_doc_pr_produces_empty_css() {
            let (doc, ns) = parse("");
            let (css, alt, title) = get_image_properties(doc.root_element(), &ns);
            assert!(css.is_empty());
            assert!(alt.is_none());
            assert!(title.is_none());
        }
    }

    #[test]
    fn get_image_margins_is_always_empty() {
        // Reproduces a real upstream bug -- see the module docs.
        let (doc, ns) =
            parse(r#"<wp:effectExtent distL="91440" distT="45720" distR="91440" distB="45720"/>"#);
        let elem = ns.children(doc.root_element(), &["wp:effectExtent"])[0];
        assert!(get_image_margins(elem).is_empty());
    }

    mod get_hpos_tests {
        use super::*;

        #[test]
        fn left_margin_relative_from_returns_width_frac() {
            let (doc, ns) = parse(r#"<wp:positionH relativeFrom="leftMargin"/>"#);
            assert_eq!(get_hpos(doc.root_element(), 400.0, &ns, 0.1), 0.1);
        }

        #[test]
        fn right_margin_relative_from_returns_one_plus_width_frac() {
            let (doc, ns) = parse(r#"<wp:positionH relativeFrom="rightMargin"/>"#);
            assert_eq!(get_hpos(doc.root_element(), 400.0, &ns, 0.1), 1.1);
        }

        #[test]
        fn a_page_relative_align_is_returned_unshifted() {
            let (doc, ns) = parse(
                r#"<wp:positionH relativeFrom="page"><wp:align>center</wp:align></wp:positionH>"#,
            );
            assert_eq!(get_hpos(doc.root_element(), 400.0, &ns, 0.1), 0.5);
        }

        #[test]
        fn a_column_relative_align_is_shifted_by_width_frac() {
            let (doc, ns) = parse(
                r#"<wp:positionH relativeFrom="column"><wp:align>right</wp:align></wp:positionH>"#,
            );
            assert_eq!(get_hpos(doc.root_element(), 400.0, &ns, 0.1), 1.1);
        }

        #[test]
        fn a_raw_offset_is_scaled_by_page_width() {
            let (doc, ns) = parse(
                r#"<wp:positionH relativeFrom="column"><wp:posOffset>127000</wp:posOffset></wp:positionH>"#,
            );
            // emu_to_pt(127000) == 10.0pt
            assert_eq!(get_hpos(doc.root_element(), 100.0, &ns, 0.0), 0.1);
        }

        #[test]
        fn no_position_h_falls_through_to_zero() {
            let (doc, ns) = parse("");
            assert_eq!(get_hpos(doc.root_element(), 400.0, &ns, 0.1), 0.0);
        }
    }

    mod get_float_properties_tests {
        use super::*;

        fn page() -> PageProperties {
            PageProperties {
                width: 612.0,
                height: 792.0,
                margin_left: 72.0,
                margin_right: 72.0,
            }
        }

        #[test]
        fn display_defaults_to_block() {
            let (doc, ns) = parse(
                r#"<wp:anchor><wp:positionH relativeFrom="page"><wp:align>left</wp:align></wp:positionH></wp:anchor>"#,
            );
            let anchor = ns.children(doc.root_element(), &["wp:anchor"])[0];
            let mut style = Css::new();
            get_float_properties(anchor, &mut style, &page(), &ns);
            assert_eq!(style.get("display").map(String::as_str), Some("block"));
        }

        #[test]
        fn a_wrap_square_on_the_left_floats_left() {
            let (doc, ns) = parse(
                r#"<wp:anchor>
                     <wp:positionH relativeFrom="page"><wp:align>left</wp:align></wp:positionH>
                     <wp:wrapSquare wrapText="bothSides"/>
                   </wp:anchor>"#,
            );
            let anchor = ns.children(doc.root_element(), &["wp:anchor"])[0];
            let mut style = Css::new();
            get_float_properties(anchor, &mut style, &page(), &ns);
            assert_eq!(style.get("float").map(String::as_str), Some("left"));
        }

        #[test]
        fn wrap_none_centers_via_auto_margins_instead_of_floating() {
            let (doc, ns) = parse(
                r#"<wp:anchor>
                     <wp:positionH relativeFrom="page"><wp:align>center</wp:align></wp:positionH>
                     <wp:wrapNone/>
                   </wp:anchor>"#,
            );
            let anchor = ns.children(doc.root_element(), &["wp:anchor"])[0];
            let mut style = Css::new();
            get_float_properties(anchor, &mut style, &page(), &ns);
            assert!(!style.contains_key("float"));
            assert_eq!(style.get("margin-left").map(String::as_str), Some("auto"));
            assert_eq!(style.get("margin-right").map(String::as_str), Some("auto"));
        }

        #[test]
        fn wrap_text_right_forces_hpos_to_zero() {
            let (doc, ns) = parse(
                r#"<wp:anchor>
                     <wp:positionH relativeFrom="page"><wp:align>right</wp:align></wp:positionH>
                     <wp:wrapSquare wrapText="right"/>
                   </wp:anchor>"#,
            );
            let anchor = ns.children(doc.root_element(), &["wp:anchor"])[0];
            let mut style = Css::new();
            get_float_properties(anchor, &mut style, &page(), &ns);
            // hpos forced to 0 (< 0.65) despite align="right" -> floats left.
            assert_eq!(style.get("float").map(String::as_str), Some("left"));
        }

        #[test]
        fn no_wrap_element_leaves_only_display_and_padding() {
            let (doc, ns) = parse(
                r#"<wp:anchor><wp:positionH relativeFrom="page"><wp:align>left</wp:align></wp:positionH></wp:anchor>"#,
            );
            let anchor = ns.children(doc.root_element(), &["wp:anchor"])[0];
            let mut style = Css::new();
            get_float_properties(anchor, &mut style, &page(), &ns);
            assert!(!style.contains_key("float"));
            assert!(!style.contains_key("margin-left"));
        }
    }
}
