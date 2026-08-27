//! Image embedding (`docx/writer/images.py`) -- **partial**: the
//! self-contained utility layer, [`ImagesManager`]'s data-source half
//! (`read_image`/`read_svg`), `create_image_markup`/`add_image` (and
//! their `from_html.rs` `<img>`-tag wiring), and now `serialize` are
//! all ported; only the cover-image methods remain.
//!
//! Ported: [`get_image_margins`], [`create_filename`], and
//! [`create_docx_image_markup`] (the actual `w:drawing`/`pic:pic`
//! OOXML builder, factored out of `create_image_markup`).
//! [`crate::oeb::polish::style::Style::img_size`] (a prerequisite
//! `create_image_markup` needs) is ported too, alongside this file
//! since it lives in `oeb/polish/style.rs`. [`Image`] and
//! [`ImagesManager`]'s `read_image`/`read_svg` resolve the real
//! image-content-source design question (an existing crate-wide
//! idiom, `OEBBook.container.read(href)` -- no new abstraction was
//! needed). [`ImagesManager::create_image_markup`]/`::add_image` are
//! real too -- the floating/margin decision logic, `wp:inline`/
//! `wp:anchor` assembly, and the `<img>` element's own `src`/`abshref`
//! resolution, using [`Floating`] (a real 3-value enum standing in for
//! Python's `'left' | 'right' | 'center' | None` string-or-None
//! value) in place of the raw string Python threads through --
//! `from_html.rs`'s `add_block_tag`/`add_inline_tag` call `add_image`
//! for real now too, via a new `images_manager`/`names` pair of fields
//! on `ProcessCtx`.
//!
//! [`ImagesManager::serialize`] is now real too -- writing every
//! embedded image's bytes into a `part name -> bytes` map, exactly the
//! shape [`super::container::DocxWriter::parts`] (this crate's `DOCX`
//! equivalent) already has, contrary to this method's own earlier doc
//! history flagging "needs write-time package access" as a blocker;
//! no new capability was actually needed. Bytes are re-read straight
//! from `self.oeb.container` at serialize time rather than cached, the
//! same "don't hold every embedded image in memory for the whole
//! conversion" property Python's lazy `partial(self.get_data, ...)`
//! callback achieves differently.
//!
//! **Not ported, each for a real reason, not oversight**:
//! - SVG-original tracking (Python's `SVGRasterizer.svg_originals`,
//!   populated when `save_svg_originals=True`) -- the FIELD is now
//!   real (`ImagesManager::svg_originals`), but nothing populates it
//!   yet, since that happens during `Convert.__call__`'s SVG
//!   rasterization pass, itself unported. This crate's
//!   [`crate::oeb::transforms::rasterize::SvgRasterizer`] is a real,
//!   already-ported port of the SAME Python class, but only of its
//!   rasterization-cache half (issue #162, for a different consumer);
//!   the `svg_originals` bookkeeping this pass needs isn't part of
//!   that port and would need adding when the rasterization pass
//!   itself is ported.
//! - `create_cover_markup`/`write_cover_block` -- need the cover-image
//!   resolution flow from `Convert.__call__` (not ported, and not
//!   separable from it the way `serialize` was -- there is no
//!   `self.cover_img`-equivalent lookup anywhere yet); the
//!   `Element::insert`-at-front capability they also need is no
//!   longer a gap ([`super::xml::Element`] gained it for `links.py`'s
//!   `LinksManager::serialize_toc`, issue #132).
//! - `ImagesManager::serialize`'s own real caller -- merging its
//!   output map into [`super::container::DocxWriter::parts`] -- is
//!   `Convert.write`'s job (not ported); `DocxWriter.parts` today is
//!   only ever populated directly by tests, confirmed by grepping
//!   every call site before scoping this file's own `serialize`.

use std::collections::HashSet;

use indexmap::IndexMap;

use calibre_utils::filenames::ascii_filename;
use calibre_utils::imghdr::identify;

use crate::docx::images::pt_to_emu;
use crate::docx::names::{DocxNamespace, SVG_BLIP_URI, USE_LOCAL_DPI_URI};
use crate::lit::urlunquote;
use crate::oeb::book::OEBBook;
use crate::oeb::polish::check::parsing::urlquote;
use crate::oeb::polish::pretty::{dom_tail, leading_text};
use crate::oeb::polish::style::Style;
use crate::oeb::transforms::filenames::abshref;
use crate::oeb::transforms::rescale::fit_image;

use super::container::DocumentRelationships;
use super::from_html::Block;
use super::xml::Element;

/// Port of the `floating` local variable's value set inside
/// `create_image_markup` -- Python leaves it as a plain `str | None`,
/// but every real value it's ever assigned is one of exactly these
/// three CSS keywords (`'left'`/`'right''` from the `float`/
/// `text-align` properties, or `'center'` from either an
/// auto-margins-on-both-sides block image or a centered ancestor's
/// `text-align`) -- a real enum matches this effort's established
/// practice of replacing a well-understood string value set with a
/// type, rather than carrying `Option<String>` through the whole
/// function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Floating {
    Left,
    Right,
    Center,
}

impl Floating {
    fn as_wp_align(self) -> &'static str {
        match self {
            Floating::Left => "left",
            Floating::Right => "right",
            Floating::Center => "center",
        }
    }
}

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
/// `create_image_markup`/`add_image` are now ported -- see their own
/// docs. **Still not ported here, deliberately**: `serialize` (needs
/// write-time access to the real docx package being assembled,
/// `DocxWriter::write`'s own still-unwired content-input path),
/// `create_cover_markup`/`write_cover_block` (need the cover-image
/// resolution flow from `Convert.__call__`, not ported). Wiring
/// `add_image` into `from_html.rs`'s own `<img>`-tag `todo!()`s is
/// ALSO deliberately left for a follow-up PR -- it needs threading a
/// new `images_manager: &mut ImagesManager` through `ProcessCtx`
/// (touching every `process_tag`/`process_item` call site, including
/// every test's), a real, separate integration step, matching this
/// effort's own established "arena+algorithms PR, then integration
/// PR" split (`tables.rs`'s `Cell`/`Row`/`Table` vs. `Blocks`' own
/// wiring, PR #342 then #344/#345; `tables.rs`'s serialize methods vs.
/// `Blocks::serialize`, PR #347 then #348).
pub struct ImagesManager<'a> {
    oeb: &'a OEBBook,
    document_relationships: DocumentRelationships,
    images: IndexMap<String, Image>,
    svg_images: IndexMap<String, Image>,
    seen_filenames: HashSet<String>,
    count: u32,
    page_width: f64,
    page_height: f64,
    /// Port of `SVGRasterizer.svg_originals` (`href of the rasterized
    /// PNG -> href of the original SVG`), populated when
    /// `save_svg_originals=True`. Nothing populates this yet -- that
    /// happens during `Convert.__call__`'s SVG-rasterization pass,
    /// itself unported -- so `create_image_markup` always finds it
    /// empty today; the field exists so the type is real and correct
    /// once that pass lands, matching how `count`/`page_width`/
    /// `page_height` were added specifically because THIS PR is their
    /// first real caller.
    svg_originals: IndexMap<String, String>,
}

impl<'a> ImagesManager<'a> {
    /// Port of `ImagesManager.__init__`. `page_width`/`page_height`
    /// stand in for `opts.output_profile.width_pts`/`.height_pts` --
    /// plain scalars rather than a whole `&Profile`, since nothing
    /// else here needs one. `log` isn't stored, matching this effort's
    /// established absence of a log sink for these writer managers.
    /// `svg_rasterizer` itself isn't stored -- only its
    /// `svg_originals` map is ever read (see that field's own docs).
    pub fn new(
        oeb: &'a OEBBook,
        document_relationships: DocumentRelationships,
        page_width: f64,
        page_height: f64,
    ) -> Self {
        ImagesManager {
            oeb,
            document_relationships,
            images: IndexMap::new(),
            svg_images: IndexMap::new(),
            seen_filenames: HashSet::new(),
            count: 0,
            page_width,
            page_height,
            svg_originals: IndexMap::new(),
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

    /// Port of `ImagesManager.create_image_markup`. `stylizer` is
    /// dropped -- Python re-derives `stylizer.style(html_img)`
    /// internally, but every real caller (`add_image`, below) already
    /// holds a `Style` for the exact same node, so `style` is taken
    /// directly instead. The ancestor's style (`pstyle`, for the
    /// "inline image alone inside a block" detection) is rebuilt via
    /// `Style::new(style.dom(), style.resolved(), style.profile(),
    /// parent)`, the same "reconstruct via `Style`'s own dom/resolved/
    /// profile accessors" technique `Blocks::end_current_block`
    /// established (PR #345).
    ///
    /// `float`/`display`/`text-align` have no dedicated `@property` on
    /// either `stylizer.py`'s base `Style` or `from_html.py`'s own
    /// subclass (confirmed by reading both), so `style['float']` etc.
    /// fall through to the raw specified-value getter --
    /// [`Style::get`] (port of `Style._get`), matching this effort's
    /// established domName-dispatch discipline. `style._get('margin-
    /// left')`/`.get('margin-right')` are the SAME raw getter, already
    /// what `_get` (not `[]`) calls for directly.
    ///
    /// "`len(parent) == 1`" is lxml's `Element.__len__`, which counts
    /// only child ELEMENTS (not text/comment nodes) -- ported as a
    /// direct filter over [`crate::dom::Dom::children`] rather than a
    /// new `Dom` method, since this is the one place that needs it.
    /// `parent.text`/`html_img.tail` are [`leading_text`]/[`dom_tail`],
    /// the same lxml text/tail bridge `process_tag` already uses.
    pub fn create_image_markup(
        &mut self,
        names: &DocxNamespace,
        style: &Style,
        href: &str,
        as_block: bool,
    ) -> Option<Element> {
        let dom = style.dom();
        let html_img = style.node;

        let mut svg_rid: Option<String> = None;
        if let Some(svghref) = self.svg_originals.get(href).cloned() {
            if let Some(si) = self.read_svg(&svghref) {
                svg_rid = Some(si.rid.clone());
            }
        }

        let mut floating = match style.get("float").as_str() {
            "left" => Some(Floating::Left),
            "right" => Some(Floating::Right),
            _ => None,
        };
        let mut as_block = as_block;
        if as_block {
            let ml = style.get("margin-left");
            let mr = style.get("margin-right");
            if ml == "auto" {
                floating = Some(if mr == "auto" {
                    Floating::Center
                } else {
                    Floating::Right
                });
            }
            if mr == "auto" {
                floating = Some(if ml == "auto" {
                    Floating::Center
                } else {
                    Floating::Right
                });
            }
        } else if let Some(parent) = dom.parent(html_img) {
            let element_child_count = dom
                .children(parent)
                .iter()
                .filter(|&&c| dom.tag(c).is_some())
                .count();
            let parent_text_empty = leading_text(dom, parent)
                .map(|t| t.trim().is_empty())
                .unwrap_or(true);
            let img_tail_empty = dom_tail(dom, html_img)
                .map(|t| t.trim().is_empty())
                .unwrap_or(true);
            if element_child_count == 1 && parent_text_empty && img_tail_empty {
                let pstyle = Style::new(dom, style.resolved(), style.profile(), parent);
                if pstyle.get("display").contains("block") {
                    as_block = true;
                    floating = match pstyle.get("float").as_str() {
                        "left" => Some(Floating::Left),
                        "right" => Some(Floating::Right),
                        _ => None,
                    };
                    if floating.is_none() {
                        floating = match pstyle.get("text-align").as_str() {
                            "center" => Some(Floating::Center),
                            "right" => Some(Floating::Right),
                            _ => None,
                        };
                    }
                    floating = Some(floating.unwrap_or(Floating::Left));
                }
            }
        }

        let fake_margins = floating.is_none();
        self.count += 1;
        let img = self.images.get(href)?;
        let name = urlunquote(href.rsplit('/').next().unwrap_or(href));
        let (w, h) = style.img_size(img.width as f64, img.height as f64);
        let (_scaled, fitted_w, fitted_h) = fit_image(w, h, self.page_width, self.page_height);
        let width = pt_to_emu(fitted_w as f64);
        let height = pt_to_emu(fitted_h as f64);

        let mut drawing = Element::new("w:drawing");
        let content = if let Some(f) = floating {
            let margins = get_image_margins(style);
            let mut anchor = Element::new("wp:anchor")
                .attr("distL", margins.left.to_string())
                .attr("distR", margins.right.to_string())
                .attr("distT", margins.top.to_string())
                .attr("distB", margins.bottom.to_string())
                .attr("simplePos", "0")
                .attr("relativeHeight", "1")
                .attr("behindDoc", "0")
                .attr("locked", "0")
                .attr("layoutInCell", "1")
                .attr("allowOverlap", "1");
            anchor.append(Element::new("wp:simplePos").attr("x", "0").attr("y", "0"));
            anchor.append(
                Element::new("wp:positionH")
                    .attr("relativeFrom", "margin")
                    .with(Element::new("wp:align").with_text(f.as_wp_align())),
            );
            anchor.append(
                Element::new("wp:positionV")
                    .attr("relativeFrom", "line")
                    .with(Element::new("wp:align").with_text("top")),
            );
            drawing.append(anchor)
        } else {
            drawing.append(Element::new("wp:inline"))
        };
        content.append(
            Element::new("wp:extent")
                .attr("cx", width.to_string())
                .attr("cy", height.to_string()),
        );
        if fake_margins {
            let margins = get_image_margins(style);
            content.append(
                Element::new("wp:effectExtent")
                    .attr("l", margins.left.to_string())
                    .attr("r", margins.right.to_string())
                    .attr("t", margins.top.to_string())
                    .attr("b", margins.bottom.to_string()),
            );
        } else {
            content.append(
                Element::new("wp:effectExtent")
                    .attr("l", "0")
                    .attr("r", "0")
                    .attr("t", "0")
                    .attr("b", "0"),
            );
        }
        if floating.is_some() {
            if as_block {
                content.append(Element::new("wp:wrapTopAndBottom"));
            } else {
                content.append(Element::new("wp:wrapSquare").attr("wrapText", "bothSides"));
            }
        }
        let alt = dom
            .node(html_img)
            .attrs
            .get("alt")
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| name.clone());
        create_docx_image_markup(
            names,
            content,
            self.count,
            &name,
            &alt,
            &img.rid,
            width,
            height,
            svg_rid.as_deref(),
        );
        Some(drawing)
    }

    /// Port of `ImagesManager.add_image`. `stylizer` is dropped for
    /// the same reason [`Self::create_image_markup`] drops it --
    /// `style` (already built by the caller for `html_img`) replaces
    /// it directly. `current_item_href` stands in for the bound
    /// `self.abshref` method (`item.abshref`, bound to the CURRENT
    /// spine item being processed, `from_html.py:493`) -- the same
    /// `current_item_href: &str` parameter `LinksManager`'s own
    /// href-resolving methods already take.
    ///
    /// Python's `try: rid = self.read_image(href).rid; except
    /// AttributeError: return` (a `None.rid` access) is just
    /// `self.read_image(&href)?` here -- `read_image` already returns
    /// `None` on the same failure.
    pub fn add_image(
        &mut self,
        names: &DocxNamespace,
        style: &Style,
        block: &mut Block,
        current_item_href: &str,
        bookmark: Option<String>,
        as_block: bool,
    ) -> Option<String> {
        let dom = style.dom();
        let html_img = style.node;
        let src = dom.node(html_img).attrs.get("src")?;
        if src.is_empty() {
            return None;
        }
        let href = abshref(current_item_href, src);
        let rid = self.read_image(&href)?.rid.clone();
        let drawing = self.create_image_markup(names, style, &href, as_block)?;
        block.add_image(drawing, bookmark);
        Some(rid)
    }

    /// Port of `ImagesManager.serialize`. `images_map` stands in for
    /// Python's `self.docx.images` -- both this and
    /// [`super::container::DocxWriter::parts`] (this crate's `DOCX`
    /// equivalent) already have exactly this shape (`part name ->
    /// bytes`, written verbatim into the zip), so despite this
    /// method's own doc history flagging "needs write-time package
    /// access" as a blocker, no new capability was actually needed --
    /// `DocxWriter.parts` already existed for embedded fonts.
    ///
    /// Python's `partial(self.get_data, img.item)` defers the actual
    /// read until zip-write time specifically so `item.data`'s
    /// in-memory cache (already dropped once by
    /// `item.unload_data_from_memory()` inside `read_image`/`read_svg`)
    /// can be re-populated lazily and dropped again afterward. This
    /// port never cached bytes in memory in the first place --
    /// [`Self::read_image`]/[`Self::read_svg`] only ever kept metadata
    /// (`Image { href, fname, .. }`) -- so re-reading straight from
    /// `self.oeb.container` here, eagerly, reproduces the same
    /// "don't hold every embedded image in memory for the whole
    /// conversion" property without needing a lazy-callback map at
    /// all. A read that fails (the source item genuinely vanished
    /// between `read_image` succeeding earlier and `serialize` running
    /// now) is skipped rather than aborting the whole document --
    /// consistent with this file's existing `read_image` treatment of
    /// a missing item as a graceful `None`, not a hard error.
    pub fn serialize(&self, images_map: &mut std::collections::BTreeMap<String, Vec<u8>>) {
        for img in self.images.values() {
            if let Ok(data) = self.oeb.container.read(&img.href) {
                images_map.insert(format!("word/{}", img.fname), data);
            }
        }
        for img in self.svg_images.values() {
            if let Ok(data) = self.oeb.container.read(&img.href) {
                images_map.insert(format!("word/{}", img.fname), data);
            }
        }
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

    fn images_manager(oeb: &OEBBook) -> ImagesManager<'_> {
        ImagesManager::new(oeb, relationships(), 600.0, 800.0)
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
        let mut mgr = images_manager(&oeb);
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
        let mut mgr = images_manager(&oeb);
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
        let mut mgr = images_manager(&oeb);
        let img = mgr.read_image("images/my pic.png").unwrap();
        assert_eq!(img.href, "images/my%20pic.png");
    }

    #[test]
    fn read_image_returns_none_for_an_unknown_href() {
        let oeb = Builder::new().build();
        let mut mgr = images_manager(&oeb);
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
        let mut mgr = images_manager(&oeb);
        assert!(mgr.read_image("images/bad.png").is_none());
    }

    #[test]
    fn read_svg_registers_a_relationship_with_unresolved_dimensions() {
        let oeb = Builder::new()
            .part("images/pic.svg", "image/svg+xml", b"<svg/>", false)
            .build();
        let mut mgr = images_manager(&oeb);
        let img = mgr.read_svg("images/pic.svg").unwrap();
        assert_eq!(img.fmt, "svg");
        assert_eq!((img.width, img.height), (-1, -1));
        assert_eq!(img.fname, "media/pic.svg");
    }

    #[test]
    fn read_svg_returns_none_for_an_unknown_href() {
        let oeb = Builder::new().build();
        let mut mgr = images_manager(&oeb);
        assert!(mgr.read_svg("nope.svg").is_none());
    }

    #[test]
    fn raster_and_svg_images_share_one_filename_dedup_pool() {
        let oeb = Builder::new()
            .part("a/pic.png", "image/png", &png_bytes(1, 1), false)
            .part("b/pic.svg", "image/svg+xml", b"<svg/>", false)
            .build();
        let mut mgr = images_manager(&oeb);
        let raster = mgr.read_image("a/pic.png").unwrap().fname.clone();
        let svg = mgr.read_svg("b/pic.svg").unwrap().fname.clone();
        assert_eq!(raster, "media/pic.png");
        assert_eq!(
            svg, "media/pic1.svg",
            "create_filename's dedup is stem-only, extension-agnostic -- \
             a same-stem svg after a png collides and gets a counter"
        );
    }

    use super::super::links::LinksManager;
    use super::super::styles::StylesManager;
    use super::super::xml::Child;
    use crate::docx::writer::container::DocumentRelationships as DocRels;

    fn text_of<'a>(el: &'a Element, name: &'a str) -> Option<&'a str> {
        el.children_named(name)
            .next()?
            .children
            .iter()
            .find_map(|c| match c {
                Child::Text(t) => Some(t.as_str()),
                _ => None,
            })
    }

    #[test]
    fn create_image_markup_builds_a_plain_inline_drawing_when_not_floated() {
        // Leading text on the <p> ("text") means the img is NOT alone
        // in its parent, so the inline-image-alone-in-a-block
        // promotion never fires, and no float is declared either.
        let dom = make("<html><body><p>text<img src=\"pic.png\"/></p></body></html>");
        let img = find(&dom, "img");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, img);
        let oeb = Builder::new()
            .part("pic.png", "image/png", &png_bytes(100, 200), false)
            .build();
        let mut mgr = images_manager(&oeb);
        mgr.read_image("pic.png").unwrap();
        let drawing = mgr
            .create_image_markup(&ns(), &style, "pic.png", false)
            .unwrap();
        assert!(drawing.children_named("wp:inline").next().is_some());
        assert!(drawing.children_named("wp:anchor").next().is_none());
    }

    #[test]
    fn create_image_markup_anchors_a_floated_image() {
        // "x" leading text on the div blocks the alone-in-a-block
        // promotion, so `floating` comes straight from the img's own
        // `float: left`.
        let dom = make("<html><body><div>x<img src=\"pic.png\"/></div></body></html>");
        let img = find(&dom, "img");
        let resolved = resolved_with(&[(img, &[("float", "left")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, img);
        let oeb = Builder::new()
            .part("pic.png", "image/png", &png_bytes(100, 200), false)
            .build();
        let mut mgr = images_manager(&oeb);
        mgr.read_image("pic.png").unwrap();
        let drawing = mgr
            .create_image_markup(&ns(), &style, "pic.png", false)
            .unwrap();
        let anchor = drawing.children_named("wp:anchor").next().unwrap();
        let position_h = anchor.children_named("wp:positionH").next().unwrap();
        assert_eq!(text_of(position_h, "wp:align"), Some("left"));
        assert!(anchor.children_named("wp:wrapSquare").next().is_some());
    }

    #[test]
    fn create_image_markup_as_block_with_auto_margins_on_both_sides_centers() {
        let dom = make("<html><body><img src=\"pic.png\"/></body></html>");
        let img = find(&dom, "img");
        let resolved =
            resolved_with(&[(img, &[("margin-left", "auto"), ("margin-right", "auto")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, img);
        let oeb = Builder::new()
            .part("pic.png", "image/png", &png_bytes(100, 200), false)
            .build();
        let mut mgr = images_manager(&oeb);
        mgr.read_image("pic.png").unwrap();
        let drawing = mgr
            .create_image_markup(&ns(), &style, "pic.png", true)
            .unwrap();
        let anchor = drawing.children_named("wp:anchor").next().unwrap();
        let position_h = anchor.children_named("wp:positionH").next().unwrap();
        assert_eq!(text_of(position_h, "wp:align"), Some("center"));
        // as_block stays true here (it was already true on entry), so
        // the wrap element is wrapTopAndBottom, not wrapSquare.
        assert!(anchor
            .children_named("wp:wrapTopAndBottom")
            .next()
            .is_some());
    }

    #[test]
    fn create_image_markup_promotes_a_lone_inline_image_via_the_parents_text_align() {
        // The img is the ONLY child of the div, with no leading/tail
        // text anywhere -- exactly the shape that triggers the
        // "inline image alone inside a block" promotion. The div
        // itself has no float, so the promoted `floating` falls
        // through to its `text-align: center`.
        let dom = make("<html><body><div><img src=\"pic.png\"/></div></body></html>");
        let div = find(&dom, "div");
        let img = find(&dom, "img");
        let resolved = resolved_with(&[(div, &[("display", "block"), ("text-align", "center")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, img);
        let oeb = Builder::new()
            .part("pic.png", "image/png", &png_bytes(100, 200), false)
            .build();
        let mut mgr = images_manager(&oeb);
        mgr.read_image("pic.png").unwrap();
        let drawing = mgr
            .create_image_markup(&ns(), &style, "pic.png", false)
            .unwrap();
        let anchor = drawing.children_named("wp:anchor").next().unwrap();
        let position_h = anchor.children_named("wp:positionH").next().unwrap();
        assert_eq!(text_of(position_h, "wp:align"), Some("center"));
        // Promotion sets the internal as_block to true, so this is a
        // wrapTopAndBottom, not a wrapSquare.
        assert!(anchor
            .children_named("wp:wrapTopAndBottom")
            .next()
            .is_some());
        // floating became Some, so effectExtent uses real zeros, not
        // faked margins.
        let extent = anchor.children_named("wp:effectExtent").next().unwrap();
        assert_eq!(extent.get("l"), Some("0"));
        assert_eq!(extent.get("t"), Some("0"));
    }

    #[test]
    fn add_image_resolves_src_via_abshref_and_attaches_an_image_run_to_the_block() {
        let dom = make("<html><body><img src=\"pic.png\"/></body></html>");
        let img = find(&dom, "img");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, img);
        let oeb = Builder::new()
            .part("pic.png", "image/png", &png_bytes(100, 200), false)
            .build();
        let mut mgr = images_manager(&oeb);
        let mut styles_mgr = StylesManager::new("en");
        let mut block = Block::new(&mut styles_mgr, &dom, img, &style, false, None, false, None);
        assert!(block.is_empty());

        let rid = mgr.add_image(&ns(), &style, &mut block, "chap1.html", None, false);
        assert!(rid.is_some());
        assert!(!block.is_empty());

        let mut body = Element::new("w:body");
        let mut lm = LinksManager::new(DocRels::new(&ns()));
        block.serialize(&mut body, &mut lm, &ns(), None, false, false);
        let p_el = body.children_named("w:p").next().unwrap();
        let r = p_el.children_named("w:r").next().unwrap();
        assert!(r.children_named("w:drawing").next().is_some());
    }

    #[test]
    fn add_image_returns_none_when_the_img_has_no_src() {
        let dom = make("<html><body><img/></body></html>");
        let img = find(&dom, "img");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, img);
        let oeb = Builder::new().build();
        let mut mgr = images_manager(&oeb);
        let mut styles_mgr = StylesManager::new("en");
        let mut block = Block::new(&mut styles_mgr, &dom, img, &style, false, None, false, None);
        let rid = mgr.add_image(&ns(), &style, &mut block, "chap1.html", None, false);
        assert!(rid.is_none());
        assert!(block.is_empty());
    }

    #[test]
    fn serialize_writes_raster_and_svg_bytes_under_word_media() {
        let oeb = Builder::new()
            .part("a/pic.png", "image/png", &png_bytes(1, 1), false)
            .part("b/pic.svg", "image/svg+xml", b"<svg/>", false)
            .build();
        let mut mgr = images_manager(&oeb);
        let raster_fname = mgr.read_image("a/pic.png").unwrap().fname.clone();
        let svg_fname = mgr.read_svg("b/pic.svg").unwrap().fname.clone();

        let mut images_map = std::collections::BTreeMap::new();
        mgr.serialize(&mut images_map);

        assert_eq!(
            images_map.get(&format!("word/{raster_fname}")),
            Some(&png_bytes(1, 1))
        );
        assert_eq!(
            images_map.get(&format!("word/{svg_fname}")),
            Some(&b"<svg/>".to_vec())
        );
    }

    #[test]
    fn serialize_with_no_images_read_writes_nothing() {
        let oeb = Builder::new().build();
        let mgr = images_manager(&oeb);
        let mut images_map = std::collections::BTreeMap::new();
        mgr.serialize(&mut images_map);
        assert!(images_map.is_empty());
    }
}
