//! Image embedding (`docx/writer/images.py`) -- **partial**: only the
//! self-contained utility layer, none of `ImagesManager` itself yet.
//!
//! Ported: [`get_image_margins`], [`create_filename`], and
//! [`create_docx_image_markup`] (the actual `w:drawing`/`pic:pic`
//! OOXML builder, factored out of `create_image_markup`, which itself
//! is NOT ported). [`crate::oeb::polish::style::Style::img_size`] (a
//! prerequisite `create_image_markup` needs) is ported too, alongside
//! this file since it lives in `oeb/polish/style.rs`.
//!
//! **Not ported, each for a real reason, not oversight**:
//! - `Image` and `ImagesManager` itself (`read_image`/`read_svg`/
//!   `add_image`/`serialize`) -- these need actual image BYTE content
//!   from the OEB manifest, and `crate::oeb::manifest::ManifestItem`
//!   has no `data` field at all (confirmed by reading it, not
//!   assumed) -- `docx::writer` has no wired path to real book
//!   content yet (`DocxWriter::write` currently only produces an
//!   empty skeleton). Whether that content should come through
//!   `oeb::polish::container::Container` (the file-backed abstraction
//!   `cascade::resolve_styles` already uses) or some new mechanism is
//!   a real design question, not a routine port.
//! - `create_image_markup` (the floating/margin decision logic and
//!   final assembly) -- needs the above `Image`/real width-height
//!   data, `self.count` (a manager-owned running counter), and
//!   `self.svg_rasterizer.svg_originals` (see below). Once the
//!   `ImagesManager` design question is settled, this can be built
//!   from [`get_image_margins`]/[`Style::img_size`]/
//!   [`create_docx_image_markup`], already ported.
//! - SVG-original tracking (Python's `SVGRasterizer.svg_originals`,
//!   populated when `save_svg_originals=True`) -- this crate's
//!   [`crate::oeb::transforms::rasterize::SvgRasterizer`] is a real,
//!   already-ported port of the SAME Python class, but only of its
//!   rasterization-cache half (issue #162, for a different consumer);
//!   the `svg_originals` bookkeeping `docx::writer` needs isn't part
//!   of that port and would need adding.
//! - `create_cover_markup`/`write_cover_block` -- need
//!   [`super::xml::Element`] to support inserting a child at the
//!   front, which it doesn't (only ever appends) -- the exact same
//!   gap already flagged for `links.py`'s `LinksManager::serialize_toc`
//!   (issue #132).

use std::collections::HashSet;

use calibre_utils::filenames::ascii_filename;

use crate::docx::images::pt_to_emu;
use crate::docx::names::{DocxNamespace, SVG_BLIP_URI, USE_LOCAL_DPI_URI};
use crate::lit::urlunquote;
use crate::oeb::polish::style::Style;

use super::xml::Element;

/// Port of `get_image_margins`'s return value. Python returns a
/// `dict` of pre-stringified EMU values keyed `distL`/`distR`/`distT`/
/// `distB` (spread directly as `wp:anchor` attributes) or, for
/// `wp:effectExtent`, re-keyed to their last character lowercased
/// (`l`/`r`/`t`/`b`) -- a real Rust struct is more natural than
/// reproducing that key-juggling, and lets a caller derive either
/// attribute-naming convention from the same four fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageMargins {
    pub left: i64,
    pub right: i64,
    pub top: i64,
    pub bottom: i64,
}

/// Port of `get_image_margins`. Python's `as_num(getattr(style, ...))`
/// defensively coerces a possibly-string CSS value to a float; every
/// `Style::padding_*`/`margin_*` accessor here already returns a
/// clean `f64` (with its own `unwrap_or(0.0)` fallback), so there is
/// nothing left for `as_num` to do -- not ported as a standalone
/// function, since this was its only call site in `images.py`.
pub fn get_image_margins(style: &Style) -> ImageMargins {
    ImageMargins {
        left: pt_to_emu(style.padding_left() + style.margin_left()),
        right: pt_to_emu(style.padding_right() + style.margin_right()),
        top: pt_to_emu(style.padding_top() + style.margin_top()),
        bottom: pt_to_emu(style.padding_bottom() + style.margin_bottom()),
    }
}

/// Port of `ImagesManager.create_filename`. `seen_filenames` stands
/// in for `self.seen_filenames` (a `HashSet<String>` of
/// already-used, lowercased filenames) -- explicit parameter rather
/// than a stored field, matching this effort's established pattern
/// (`Blocks`, `StylesManager`, ...) for state a not-yet-built
/// `ImagesManager` will eventually own.
pub fn create_filename(seen_filenames: &mut HashSet<String>, href: &str, fmt: &str) -> String {
    let basename = href.rsplit('/').next().unwrap_or(href);
    let decoded = urlunquote(basename);
    let mut fname = ascii_filename(&decoded);
    // Port of posixpath.splitext: split at the last '.', but not one
    // at position 0 (a leading dot doesn't start an extension) --
    // narrower than posixpath's real "skip all leading dots" rule,
    // a disclosed simplification for a href-derived filename where a
    // multi-leading-dot name is not a realistic input.
    if let Some(dot) = fname.rfind('.') {
        if dot > 0 {
            fname.truncate(dot);
        }
    }
    fname = fname.chars().take(75).collect();
    fname = fname.trim_end_matches('.').to_string();
    if fname.is_empty() {
        fname = "image".to_string();
    }
    let base = fname.clone();
    let mut num = 0u32;
    while seen_filenames.contains(&fname.to_lowercase()) {
        num += 1;
        fname = format!("{base}{num}");
    }
    seen_filenames.insert(fname.to_lowercase());
    format!("{fname}.{}", fmt.to_lowercase())
}

/// Port of `ImagesManager.create_docx_image_markup`: the `wp:docPr`
/// through `pic:spPr` shape shared by both an inline/floating `<img>`
/// (`create_image_markup`, not yet ported) and the cover image
/// (`create_cover_markup`, not yet ported). `count` stands in for
/// `self.count` (a manager-owned running id counter across every
/// image embedded in the document).
#[allow(clippy::too_many_arguments)]
pub fn create_docx_image_markup(
    names: &DocxNamespace,
    parent: &mut Element,
    count: u32,
    name: &str,
    alt: &str,
    img_rid: &str,
    width: i64,
    height: i64,
    svg_rid: Option<&str>,
) {
    parent.append(
        Element::new("wp:docPr")
            .attr("id", count.to_string())
            .attr("name", name)
            .attr("descr", alt),
    );
    parent.append(
        Element::new("wp:cNvGraphicFramePr")
            .with(Element::new("a:graphicFrameLocks").attr("noChangeAspect", "1")),
    );
    let pic_uri = names.namespace("pic").unwrap_or_default().to_string();
    let g = parent.append(Element::new("a:graphic"));
    let gd = g.append(Element::new("a:graphicData").attr("uri", pic_uri));
    let pic = gd.append(Element::new("pic:pic"));
    let nv_pic_pr = pic.append(Element::new("pic:nvPicPr"));
    nv_pic_pr.append(
        Element::new("pic:cNvPr")
            .attr("id", "0")
            .attr("name", name)
            .attr("descr", alt),
    );
    nv_pic_pr.append(Element::new("pic:cNvPicPr"));
    let bf = pic.append(Element::new("pic:blipFill"));
    let blip = bf.append(Element::new("a:blip").attr("r:embed", img_rid));
    if let Some(svg_rid) = svg_rid {
        let ext_list = blip.append(Element::new("a:extLst"));
        ext_list.append(
            Element::new("a:ext")
                .attr("uri", USE_LOCAL_DPI_URI)
                .with(Element::new("a14:useLocalDpi").attr("val", "0")),
        );
        ext_list.append(
            Element::new("a:ext")
                .attr("uri", SVG_BLIP_URI)
                .with(Element::new("asvg:svgBlip").attr("r:embed", svg_rid)),
        );
    }
    bf.append(Element::new("a:stretch").with(Element::new("a:fillRect")));
    let sp_pr = pic.append(Element::new("pic:spPr"));
    let xfrm = sp_pr.append(Element::new("a:xfrm"));
    xfrm.append(Element::new("a:off").attr("x", "0").attr("y", "0"));
    xfrm.append(
        Element::new("a:ext")
            .attr("cx", width.to_string())
            .attr("cy", height.to_string()),
    );
    sp_pr.append(
        Element::new("a:prstGeom")
            .attr("prst", "rect")
            .with(Element::new("a:avLst")),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::Dom;
    use crate::oeb::polish::cascade::{PropertyValue, ResolvedStyles};
    use crate::oeb::polish::style::Profile;
    use std::collections::HashMap;

    fn make(html: &str) -> Dom {
        Dom::parse(html)
    }

    fn resolved_with(entries: &[(crate::dom::NodeId, &[(&str, &str)])]) -> ResolvedStyles {
        let mut style_map = HashMap::new();
        for &(id, props) in entries {
            let mut m = HashMap::new();
            for &(k, v) in props {
                m.insert(k.to_string(), PropertyValue::new(v, None, false));
            }
            style_map.insert(id, m);
        }
        ResolvedStyles {
            style_map,
            pseudo_style_map: HashMap::new(),
        }
    }

    fn find(dom: &Dom, tag: &str) -> crate::dom::NodeId {
        dom.preorder_elements(dom.root)
            .into_iter()
            .find(|&id| dom.tag(id) == Some(tag))
            .unwrap()
    }

    fn ns() -> DocxNamespace {
        DocxNamespace::new(true)
    }

    #[test]
    fn get_image_margins_sums_padding_and_margin_in_emu() {
        let dom = make("<html><body><img/></body></html>");
        let img = find(&dom, "img");
        let resolved = resolved_with(&[(
            img,
            &[
                ("padding-left", "1pt"),
                ("margin-left", "2pt"),
                ("padding-top", "3pt"),
                ("margin-top", "4pt"),
            ],
        )]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, img);
        let margins = get_image_margins(&style);
        assert_eq!(margins.left, pt_to_emu(3.0));
        assert_eq!(margins.top, pt_to_emu(7.0));
        assert_eq!(margins.right, 0);
        assert_eq!(margins.bottom, 0);
    }

    #[test]
    fn create_filename_uses_the_basename_ascii_and_lowercased_format() {
        let mut seen = HashSet::new();
        let fname = create_filename(&mut seen, "images/Caf%C3%A9.PNG", "PNG");
        assert_eq!(fname, "Cafe.png");
    }

    #[test]
    fn create_filename_strips_query_free_extension_before_appending_the_real_one() {
        let mut seen = HashSet::new();
        let fname = create_filename(&mut seen, "path/photo.jpeg", "jpg");
        assert_eq!(fname, "photo.jpg");
    }

    #[test]
    fn create_filename_dedupes_by_appending_a_counter() {
        let mut seen = HashSet::new();
        let a = create_filename(&mut seen, "dir/pic.png", "png");
        let b = create_filename(&mut seen, "other/pic.png", "png");
        let c = create_filename(&mut seen, "third/PIC.png", "png");
        assert_eq!(a, "pic.png");
        assert_eq!(b, "pic1.png");
        // The seen-set membership check is case-insensitive, but the
        // returned filename keeps its own original case (matching
        // Python: `fname.lower() in self.seen_filenames`, but
        // `fname = base + str(num)` uses the un-lowered `base`).
        assert_eq!(c, "PIC2.png");
    }

    #[test]
    fn create_filename_falls_back_to_image_when_the_stem_is_empty() {
        let mut seen = HashSet::new();
        // A trailing slash makes the basename an empty string, which
        // stays empty all the way through ascii_filename/splitext.
        let fname = create_filename(&mut seen, "dir/", "png");
        assert_eq!(fname, "image.png");
    }

    #[test]
    fn create_filename_truncates_long_stems_to_75_chars() {
        let mut seen = HashSet::new();
        let long_name = format!("{}.png", "x".repeat(100));
        let href = format!("dir/{long_name}");
        let fname = create_filename(&mut seen, &href, "png");
        assert_eq!(fname, format!("{}.png", "x".repeat(75)));
    }

    #[test]
    fn create_docx_image_markup_embeds_the_relationship_id_and_size() {
        let names = ns();
        let mut parent = Element::new("wp:inline");
        create_docx_image_markup(
            &names,
            &mut parent,
            1,
            "pic.png",
            "alt text",
            "rId5",
            100,
            200,
            None,
        );
        let doc_pr = parent.children_named("wp:docPr").next().unwrap();
        assert_eq!(doc_pr.get("id"), Some("1"));
        assert_eq!(doc_pr.get("name"), Some("pic.png"));
        assert_eq!(doc_pr.get("descr"), Some("alt text"));

        let graphic = parent.children_named("a:graphic").next().unwrap();
        let graphic_data = graphic.children_named("a:graphicData").next().unwrap();
        assert_eq!(
            graphic_data.get("uri"),
            Some("http://schemas.openxmlformats.org/drawingml/2006/picture")
        );
        let pic = graphic_data.children_named("pic:pic").next().unwrap();
        let blip_fill = pic.children_named("pic:blipFill").next().unwrap();
        let blip = blip_fill.children_named("a:blip").next().unwrap();
        assert_eq!(blip.get("r:embed"), Some("rId5"));
        assert!(blip.children_named("a:extLst").next().is_none());

        let sp_pr = pic.children_named("pic:spPr").next().unwrap();
        let xfrm = sp_pr.children_named("a:xfrm").next().unwrap();
        let ext = xfrm.children_named("a:ext").next().unwrap();
        assert_eq!(ext.get("cx"), Some("100"));
        assert_eq!(ext.get("cy"), Some("200"));
    }

    #[test]
    fn create_docx_image_markup_with_an_svg_rid_adds_the_ext_list() {
        let names = ns();
        let mut parent = Element::new("wp:inline");
        create_docx_image_markup(
            &names,
            &mut parent,
            2,
            "pic.svg",
            "alt",
            "rId5",
            100,
            200,
            Some("rId9"),
        );
        let blip = parent
            .children_named("a:graphic")
            .next()
            .unwrap()
            .children_named("a:graphicData")
            .next()
            .unwrap()
            .children_named("pic:pic")
            .next()
            .unwrap()
            .children_named("pic:blipFill")
            .next()
            .unwrap()
            .children_named("a:blip")
            .next()
            .unwrap();
        let ext_list = blip.children_named("a:extLst").next().unwrap();
        let exts: Vec<_> = ext_list.children_named("a:ext").collect();
        assert_eq!(exts.len(), 2);
        assert_eq!(exts[0].get("uri"), Some(USE_LOCAL_DPI_URI));
        assert_eq!(exts[1].get("uri"), Some(SVG_BLIP_URI));
        let svg_blip = exts[1].children_named("asvg:svgBlip").next().unwrap();
        assert_eq!(svg_blip.get("r:embed"), Some("rId9"));
    }
}
