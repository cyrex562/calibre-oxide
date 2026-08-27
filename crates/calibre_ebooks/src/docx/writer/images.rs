//! Image embedding (`docx/writer/images.py`) -- **partial**: the
//! self-contained utility layer AND [`ImagesManager`]'s data-source
//! half (`read_image`/`read_svg`) are ported; `create_image_markup`/
//! `add_image`/`serialize`/the cover-image methods are not.
//!
//! Ported: [`get_image_margins`], [`create_filename`], and
//! [`create_docx_image_markup`] (the actual `w:drawing`/`pic:pic`
//! OOXML builder, factored out of `create_image_markup`, which itself
//! is NOT ported). [`crate::oeb::polish::style::Style::img_size`] (a
//! prerequisite `create_image_markup` needs) is ported too, alongside
//! this file since it lives in `oeb/polish/style.rs`. [`Image`] and
//! [`ImagesManager`]'s `read_image`/`read_svg` are now ported too --
//! see [`ImagesManager`]'s own docs for the real image-content-source
//! design question this resolves (an existing crate-wide idiom,
//! `OEBBook.container.read(href)` -- no new abstraction was needed).
//!
//! **Not ported, each for a real reason, not oversight**:
//! - `create_image_markup` (the floating/margin decision logic and
//!   final assembly) -- needs a `Style`/stylizer context on top of
//!   the already-ported [`get_image_margins`]/[`Style::img_size`]/
//!   [`create_docx_image_markup`], plus `self.count` (a manager-owned
//!   running counter, not yet added since nothing calls it) and
//!   `self.svg_rasterizer.svg_originals` (see below).
//! - `add_image` -- needs a real DOM `<img>`/[`super::from_html::Block`]
//!   to attach to, part of the still-unported `from_html.py`
//!   `<img>`-tag `todo!()` gaps.
//! - SVG-original tracking (Python's `SVGRasterizer.svg_originals`,
//!   populated when `save_svg_originals=True`) -- this crate's
//!   [`crate::oeb::transforms::rasterize::SvgRasterizer`] is a real,
//!   already-ported port of the SAME Python class, but only of its
//!   rasterization-cache half (issue #162, for a different consumer);
//!   the `svg_originals` bookkeeping `docx::writer` needs isn't part
//!   of that port and would need adding.
//! - `serialize` -- needs write-time access to the real docx package
//!   being assembled, part of `DocxWriter::write`'s own still-unwired
//!   content-input path (`Convert.__call__`, not ported).
//! - `create_cover_markup`/`write_cover_block` -- need the cover-image
//!   resolution flow from `Convert.__call__` (not ported); the
//!   `Element::insert`-at-front capability they also need is no
//!   longer a gap ([`super::xml::Element`] gained it for `links.py`'s
//!   `LinksManager::serialize_toc`, issue #132).

use std::collections::HashSet;

use indexmap::IndexMap;

use calibre_utils::filenames::ascii_filename;
use calibre_utils::imghdr::identify;

use crate::docx::images::pt_to_emu;
use crate::docx::names::{DocxNamespace, SVG_BLIP_URI, USE_LOCAL_DPI_URI};
use crate::lit::urlunquote;
use crate::oeb::book::OEBBook;
use crate::oeb::polish::check::parsing::urlquote;
use crate::oeb::polish::style::Style;

use super::container::DocumentRelationships;
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

/// Port of the `Image` namedtuple (`rid fname width height fmt item`).
/// `href` replaces Python's `item` -- every real reader of `img.item`
/// (`serialize`, not yet ported) only ever needs it to re-read the
/// manifest item's raw bytes, which [`ImagesManager`]'s container
/// access can do from the href alone; narrower than holding a whole
/// `ManifestItem`, matching this effort's established pattern
/// (`TextRun.link`, PR #332).
#[derive(Debug, Clone)]
pub struct Image {
    pub rid: String,
    pub fname: String,
    pub width: i64,
    pub height: i64,
    pub fmt: String,
    pub href: String,
}

/// Reads calibre's bundled `images/blank.png` (the corrupted-image
/// fallback) -- port of `I('blank.png', data=True, ...)`.
/// `allow_user_override=False` in Python maps to `false` here,
/// matching the one real call site.
fn read_blank_png() -> Option<Vec<u8>> {
    let path = calibre_utils::resources::get_image_path("blank.png", false)?;
    std::fs::read(path).ok()
}

/// Port of `ImagesManager`'s data-source half: `read_image`/
/// `read_svg`, and the `images`/`svg_images`/`seen_filenames`/`count`
/// state backing them.
///
/// **The real design question this type resolves**: where do real
/// image bytes come from? Answer, confirmed by grepping every other
/// writer in this crate (`mobi::writer2::serializer`,
/// `mobi::writer8::{main,toc}`, `oeb::transforms::{guide,data_url,
/// htmltoc,metadata}`, `input::{rb,snb}_input`) -- they all read a
/// manifest item's raw bytes via `OEBBook.container: Box<dyn
/// crate::oeb::container::Container>`'s `.read(href)`, the SAME
/// abstraction `oeb::reader.rs` itself uses to load the OPF. This is
/// NOT `oeb::polish::container::Container` (the filesystem-backed
/// "Polish Book" editor, issue #163's own `Container`, a different
/// type despite the identical name) -- confirmed by reading both
/// definitions before picking one, not assumed from the name alone.
/// No new abstraction was needed at all.
///
/// `oeb`/`document_relationships` are stored as fields, unlike
/// `StylesManager`/`LinksManager`'s managers-as-explicit-parameters
/// pattern -- that pattern exists specifically to avoid storing `&mut`
/// references that would alias with OTHER managers' own `&mut`
/// borrows of the same subsystem; `oeb: &'a OEBBook` here is a plain
/// immutable borrow (no aliasing risk), and `document_relationships`
/// is owned by value, mirroring [`super::links::LinksManager`]'s own
/// field (PR #331) -- Python shares one `document_relationships`
/// object by reference across every manager; how that reconciles with
/// each Rust manager owning its own copy is `Convert`'s orchestration
/// question, not this type's, and is left for whichever PR wires
/// `Convert.__call__` together.
///
/// **Not ported here, deliberately**: `add_image` (needs a real DOM
/// `<img>`/[`super::from_html::Block`] to attach to), `create_image_markup`
/// (needs a `Style`/floating-position decision on top of the
/// already-ported [`get_image_margins`]/[`create_docx_image_markup`]/
/// [`crate::oeb::polish::style::Style::img_size`], plus
/// `SvgRasterizer.svg_originals` tracking -- still not part of this
/// crate's `SvgRasterizer` port, issue #162), `serialize` (needs
/// write-time access to the real docx package being assembled,
/// `DocxWriter::write`'s own still-unwired content-input path),
/// `create_cover_markup`/`write_cover_block` (need the cover-image
/// resolution flow from `Convert.__call__`, not ported).
pub struct ImagesManager<'a> {
    oeb: &'a OEBBook,
    document_relationships: DocumentRelationships,
    images: IndexMap<String, Image>,
    svg_images: IndexMap<String, Image>,
    seen_filenames: HashSet<String>,
}

impl<'a> ImagesManager<'a> {
    /// Port of `ImagesManager.__init__`, minus `page_width`/
    /// `page_height`/`svg_rasterizer` -- neither has a real caller yet
    /// (both belong to `create_image_markup`, not ported); `log` isn't
    /// stored, matching this effort's established absence of a log
    /// sink for these writer managers so far; `count` isn't stored
    /// either, for the same "no real caller yet" reason (only
    /// `create_image_markup`/`create_docx_image_markup` read it, and
    /// the latter already takes it as a plain parameter, PR #337).
    pub fn new(oeb: &'a OEBBook, document_relationships: DocumentRelationships) -> Self {
        ImagesManager {
            oeb,
            document_relationships,
            images: IndexMap::new(),
            svg_images: IndexMap::new(),
            seen_filenames: HashSet::new(),
        }
    }

    pub fn document_relationships(&self) -> &DocumentRelationships {
        &self.document_relationships
    }

    /// Port of `self.oeb.manifest.hrefs.get(href) or
    /// self.oeb.manifest.hrefs.get(urlquote(href))`: the found item's
    /// OWN `.href` (the canonical form stored as the manifest key),
    /// not necessarily `href` itself -- a quoted-form match returns
    /// the quoted href, matching Python's `item.href` (the resolved
    /// object), not the caller's original lookup string.
    fn find_manifest_href(&self, href: &str) -> Option<String> {
        self.oeb
            .manifest
            .get_by_href(href)
            .or_else(|| self.oeb.manifest.get_by_href(&urlquote(href)))
            .map(|item| item.href.clone())
    }

    /// Port of `ImagesManager.read_svg`. Dimensions are always `-1`
    /// (Python never resolves an SVG's real size at this point either
    /// -- that happens later, in the still-unported rasterization
    /// path).
    pub fn read_svg(&mut self, href: &str) -> Option<&Image> {
        if !self.svg_images.contains_key(href) {
            let real_href = self.find_manifest_href(href)?;
            let image_fname = format!(
                "media/{}",
                create_filename(&mut self.seen_filenames, href, "svg")
            );
            let image_rid = self.document_relationships.add_image(&image_fname);
            self.svg_images.insert(
                href.to_string(),
                Image {
                    rid: image_rid,
                    fname: image_fname,
                    width: -1,
                    height: -1,
                    fmt: "svg".to_string(),
                    href: real_href,
                },
            );
        }
        self.svg_images.get(href)
    }

    /// Port of `ImagesManager.read_image`. `item.data`'s `isinstance(...,
    /// bytes)` guard has no equivalent here -- `Container::read`
    /// always returns raw bytes regardless of media type (nothing
    /// parses image items into a DOM the way HTML/XML items are), so
    /// there's no "wrong Python type" case to guard against. The
    /// corrupted-image fallback fires when `identify` doesn't
    /// recognize the format at all (`fmt.is_none()`) -- Python's
    /// `identify` never raises either; its own `try/except` is
    /// defensive against a failure mode this port's `identify`
    /// doesn't have.
    pub fn read_image(&mut self, href: &str) -> Option<&Image> {
        if !self.images.contains_key(href) {
            let real_href = self.find_manifest_href(href)?;
            let mut data = self.oeb.container.read(&real_href).ok()?;
            let (fmt, mut width, mut height) = identify(&data);
            let fmt = match fmt {
                Some(f) => f.to_string(),
                None => {
                    data = read_blank_png()?;
                    let (fallback_fmt, w, h) = identify(&data);
                    width = w;
                    height = h;
                    fallback_fmt?.to_string()
                }
            };
            let image_fname = format!(
                "media/{}",
                create_filename(&mut self.seen_filenames, href, &fmt)
            );
            let image_rid = self.document_relationships.add_image(&image_fname);
            self.images.insert(
                href.to_string(),
                Image {
                    rid: image_rid,
                    fname: image_fname,
                    width,
                    height,
                    fmt,
                    href: real_href,
                },
            );
        }
        self.images.get(href)
    }
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

    use super::super::container::DocumentRelationships;
    use crate::oeb::transforms::test_support::Builder;

    fn relationships() -> DocumentRelationships {
        DocumentRelationships::new(&ns())
    }

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
        data.extend_from_slice(&[0, 0, 0, 13]);
        data.extend_from_slice(b"IHDR");
        data.extend_from_slice(&width.to_be_bytes());
        data.extend_from_slice(&height.to_be_bytes());
        data
    }

    #[test]
    fn read_image_reads_bytes_via_the_container_and_identifies_format() {
        let oeb = Builder::new()
            .part("images/pic.png", "image/png", &png_bytes(100, 200), false)
            .build();
        let mut mgr = ImagesManager::new(&oeb, relationships());
        let img = mgr.read_image("images/pic.png").unwrap();
        assert_eq!(img.fmt, "png");
        assert_eq!((img.width, img.height), (100, 200));
        assert_eq!(img.fname, "media/pic.png");
        assert!(!img.rid.is_empty());
    }

    #[test]
    fn read_image_caches_and_reuses_the_relationship_id_on_repeat_calls() {
        let oeb = Builder::new()
            .part("images/pic.png", "image/png", &png_bytes(1, 1), false)
            .build();
        let mut mgr = ImagesManager::new(&oeb, relationships());
        let rid1 = mgr.read_image("images/pic.png").unwrap().rid.clone();
        let rid2 = mgr.read_image("images/pic.png").unwrap().rid.clone();
        assert_eq!(rid1, rid2);
    }

    #[test]
    fn read_image_falls_back_through_a_urlquoted_manifest_href() {
        // The manifest stores the already-escaped href; a caller
        // looking it up with the raw, unescaped form should still find
        // it via the urlquote(href) fallback lookup.
        let oeb = Builder::new()
            .part("images/my%20pic.png", "image/png", &png_bytes(1, 1), false)
            .build();
        let mut mgr = ImagesManager::new(&oeb, relationships());
        let img = mgr.read_image("images/my pic.png").unwrap();
        assert_eq!(img.href, "images/my%20pic.png");
    }

    #[test]
    fn read_image_returns_none_for_an_unknown_href() {
        let oeb = Builder::new().build();
        let mut mgr = ImagesManager::new(&oeb, relationships());
        assert!(mgr.read_image("nope.png").is_none());
    }

    #[test]
    fn read_image_gracefully_returns_none_when_unrecognized_and_no_blank_png_fallback_is_available()
    {
        // A real calibre install falls back to a bundled `images/blank.png`
        // resource for a corrupted/unrecognized image (`I('blank.png', ...)`
        // in Python). This test environment has no CALIBRE_RESOURCES_PATH
        // configured, so that resource genuinely isn't resolvable here --
        // read_image should degrade to None rather than panicking.
        let oeb = Builder::new()
            .part("images/bad.png", "image/png", b"not an image", false)
            .build();
        let mut mgr = ImagesManager::new(&oeb, relationships());
        assert!(mgr.read_image("images/bad.png").is_none());
    }

    #[test]
    fn read_svg_registers_a_relationship_with_unresolved_dimensions() {
        let oeb = Builder::new()
            .part("images/pic.svg", "image/svg+xml", b"<svg/>", false)
            .build();
        let mut mgr = ImagesManager::new(&oeb, relationships());
        let img = mgr.read_svg("images/pic.svg").unwrap();
        assert_eq!(img.fmt, "svg");
        assert_eq!((img.width, img.height), (-1, -1));
        assert_eq!(img.fname, "media/pic.svg");
    }

    #[test]
    fn read_svg_returns_none_for_an_unknown_href() {
        let oeb = Builder::new().build();
        let mut mgr = ImagesManager::new(&oeb, relationships());
        assert!(mgr.read_svg("nope.svg").is_none());
    }

    #[test]
    fn raster_and_svg_images_share_one_filename_dedup_pool() {
        let oeb = Builder::new()
            .part("a/pic.png", "image/png", &png_bytes(1, 1), false)
            .part("b/pic.svg", "image/svg+xml", b"<svg/>", false)
            .build();
        let mut mgr = ImagesManager::new(&oeb, relationships());
        let raster = mgr.read_image("a/pic.png").unwrap().fname.clone();
        let svg = mgr.read_svg("b/pic.svg").unwrap().fname.clone();
        assert_eq!(raster, "media/pic.png");
        assert_eq!(
            svg, "media/pic1.svg",
            "create_filename's dedup is stem-only, extension-agnostic -- \
             a same-stem svg after a png collides and gets a counter"
        );
    }
}
