//! Port of `old_src/src/calibre/ebooks/docx/images.py`.
//!
//! Two halves, ported across two issues:
//!
//! - **Geometry/CSS** (#289's first PR): filename sanitizing,
//!   EMU-to-point conversion, and the three functions that compute an
//!   image's CSS (`get_image_properties`, `get_image_margins`,
//!   `get_hpos`/`get_float_properties`) -- no filesystem access, no
//!   `crate::dom` output.
//! - **The [`Images`] struct** (this PR, #289): real embedded-image
//!   extraction (from the DOCX zip, or a `file://`-linked path),
//!   deduplicated unique naming, and optional resizing, writing to
//!   `dest_dir/images/`. The `w:drawing`/`w:pict` -> `<img>` markup
//!   generators (`pic_to_img`, `drawing_to_html`, `pict_to_html`,
//!   `to_html`) are still a separate, smaller follow-up -- everything
//!   *they* need (`generate_filename` and friends) is done here.
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
//!
//! # A disclosed gap: EMF images
//!
//! Python's `read_image_data` tries to convert an embedded EMF
//! (Enhanced Metafile, a Windows vector format some older Word
//! documents embed) to a raster PNG via `calibre.utils.wmf.emf.emf_unwrap`
//! before writing it out. No Rust EMF parser exists anywhere in this
//! crate, so [`read_image_data`] returns the raw EMF bytes unconverted
//! (`ext` stays `"emf"`) -- an e-reader almost certainly can't display
//! that, but this is at least the same "silently give up" outcome
//! Python's own `except Exception: self.log.exception(...)` fallback
//! produces when `emf_unwrap` itself fails.
//!
//! # Not (yet) wired up: `numbering.py`'s picture-bullet CSS
//!
//! `Level.css(images, pic_map, rid_map)` (Python's `numbering.py`) is
//! the *other* real caller of `generate_filename` in the whole module
//! -- with `max_width`/`max_height` set, unlike either in-file call
//! site here, which is why [`resize_image`] is a real, exercised path
//! despite looking unused from this file alone. Wiring that up needs
//! `pic_map` construction (`w:numPicBullet` reading, not yet ported)
//! and a signature change to the already-shipped `Level::css`
//! (`numbering.rs`) -- a separate follow-up.

use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::Path;
use std::sync::OnceLock;

use calibre_utils::filenames::{ascii_filename, sanitize_file_name};
use image::GenericImageView;
use regex::Regex;
use roxmltree::Node;

use super::block_styles::{format_g, pt, Css};
use super::container::Docx;
use super::names::{DocxNamespace, SVG_BLIP_URI};
use super::styles::PageProperties;
use crate::dom::{Dom, NodeId};
use crate::oeb::transforms::rescale::fit_image;

/// `'; '.join(f'{k}: {v}' for k, v in style.items())` -- note the space
/// after the colon; [`pict_to_html`]'s `<hr>` styling uses the same
/// join but *without* that space (`f'{k}:{v}'`), so this isn't reused
/// there.
fn css_to_style_string(css: &Css) -> String {
    css.iter()
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join("; ")
}

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

/// A linked (`file://`) or embedded (zip-relationship) image whose
/// data couldn't be read. Port of `LinkedImageNotFound`; `fname` is
/// the path (linked) or relationship id's resolved zip path (embedded)
/// that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedImageNotFound {
    pub fname: String,
}

/// Reads `fname`'s raw bytes -- from the local filesystem if it's a
/// `file://` URI (a linked, not embedded, image), else from `docx` --
/// and derives a `stem.ext` base filename for it: `base` if given,
/// else `fname`'s own last path segment sanitized via
/// [`image_filename`], else `"image"`; extension from
/// [`calibre_utils::imghdr::what`] if the format is recognized, else
/// `base`'s own extension, else `"jpeg"`. See the module docs for why
/// an EMF image's bytes are returned as-is rather than converted to a
/// raster format.
///
/// Port of `Images.read_image_data`.
fn read_image_data<R: Read + Seek>(
    docx: &mut Docx<R>,
    fname: &str,
    base: Option<&str>,
) -> Result<(Vec<u8>, String), LinkedImageNotFound> {
    let raw = if let Some(rest) = fname.strip_prefix("file://") {
        let mut src = rest.to_string();
        if cfg!(windows) && src.starts_with('/') {
            src = src[1..].to_string();
        }
        if src.is_empty() || !Path::new(&src).exists() {
            return Err(LinkedImageNotFound { fname: src });
        }
        std::fs::read(&src).map_err(|_| LinkedImageNotFound { fname: src })?
    } else {
        docx.read(fname).map_err(|_| LinkedImageNotFound {
            fname: fname.to_string(),
        })?
    };

    let base = base
        .map(str::to_string)
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| {
            let last_segment = fname.rsplit('/').next().unwrap_or(fname);
            let f = image_filename(last_segment);
            if f.is_empty() {
                "image".to_string()
            } else {
                f
            }
        });

    let ext = calibre_utils::imghdr::what(&raw)
        .map(str::to_string)
        .or_else(|| {
            Some(match base.rsplit_once('.') {
                Some((_, e)) => e.to_string(),
                None => base.clone(),
            })
        })
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "jpeg".to_string());

    let stem = match base.rsplit_once('.') {
        Some((s, _)) => s.to_string(),
        None => String::new(),
    };
    let stem = if stem.is_empty() {
        "image".to_string()
    } else {
        stem
    };

    Ok((raw, format!("{stem}.{ext}")))
}

fn split_ext(base: &str) -> (String, String) {
    match base.rsplit_once('.') {
        Some((n, e)) if !n.is_empty() => (n.to_string(), format!(".{e}")),
        _ => (base.to_string(), String::new()),
    }
}

fn image_format_for_ext(ext: &str) -> image::ImageFormat {
    match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "png" => image::ImageFormat::Png,
        "gif" => image::ImageFormat::Gif,
        _ => image::ImageFormat::Jpeg,
    }
}

/// Shrinks `raw` to fit inside `max_width`x`max_height` (preserving
/// aspect ratio, via the same [`fit_image`] `oeb::transforms::rescale`
/// uses) if it doesn't already, re-encoding in the format `base`'s own
/// extension names. Returns `(possibly-resized bytes, possibly
/// `-WxH`-suffixed base filename, whether a resize actually happened)`.
/// An undecodable image is treated as "no resize needed" rather than
/// propagating a decode error -- this file's own two call sites never
/// pass a size limit at all, and its one real caller
/// (`numbering.py`'s `Level.css`, not yet wired up) already wraps the
/// whole `generate_filename` call in a broad `except Exception:
/// fname = None`, so nothing downstream distinguishes "not resized
/// because already small enough" from "not resized because it
/// couldn't be decoded" -- both mean "use the image as originally
/// read".
///
/// Port of `Images.resize_image`.
fn resize_image(
    raw: &[u8],
    base: &str,
    max_width: u32,
    max_height: u32,
) -> (Vec<u8>, String, bool) {
    let Ok(img) = image::load_from_memory(raw) else {
        return (raw.to_vec(), base.to_string(), false);
    };
    let (w, h) = img.dimensions();
    let (resized, nw, nh) = fit_image(w as f64, h as f64, max_width as f64, max_height as f64);
    if !resized {
        return (raw.to_vec(), base.to_string(), false);
    }
    let nw = (nw.max(1)) as u32;
    let nh = (nh.max(1)) as u32;
    let resized_img = img.resize_exact(nw, nh, image::imageops::FilterType::Lanczos3);

    let (stem, ext) = split_ext(base);
    let new_base = format!("{stem}-{max_width}x{max_height}{ext}");
    let mut buf = std::io::Cursor::new(Vec::new());
    if resized_img
        .write_to(&mut buf, image_format_for_ext(&ext))
        .is_err()
    {
        return (raw.to_vec(), base.to_string(), false);
    }
    (buf.into_inner(), new_base, true)
}

/// One image's hyperlink, recorded by [`Images::pic_to_img`] for
/// [`super::to_html::resolve_links`] to wrap the `<img>` in a real
/// `<a>` later. Port of the `{'id':..., 'target':..., 'title':...}`
/// dict Python builds from a `w:drawing`'s `a:hlinkClick`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageLink {
    pub id: String,
    pub target: Option<String>,
    pub title: Option<String>,
}

/// Embedded-image extraction, deduplicated naming, optional resizing,
/// disk-writing, and `w:drawing`/`w:pict` -> `<img>` markup generation,
/// for one document's worth of image references. Port of the `Images`
/// class -- minus `namespace`/`log`/`rid_map` (this port threads
/// relationships through as an explicit parameter everywhere, not
/// mutable instance state, matching every other function in
/// `to_html.rs`), and minus `names`/`resized` (write-only fields in
/// Python, never read anywhere in the file).
#[derive(Debug, Clone, Default)]
pub struct Images {
    used: HashMap<(String, Option<(u32, u32)>), String>,
    all_images: std::collections::HashSet<String>,
    /// Every image found wrapped in a hyperlink, in the order
    /// [`Images::pic_to_img`] created them, alongside the
    /// relationships map that was current when it was recorded (a
    /// footnote's images are recorded against that note's *own*
    /// relationships, not the main document's -- matching Python's
    /// per-tuple `self.rid_map` snapshot, since `self.rid_map` itself
    /// is swapped out and restored around footnote conversion).
    /// Consumed by [`super::to_html::resolve_links`]. Port of
    /// `self.links`.
    pub links: Vec<(NodeId, ImageLink, HashMap<String, String>)>,
}

impl Images {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every generated filename, `images/`-prefixed -- for the
    /// not-yet-wired-up OPF manifest step `Convert.write` (issue #288)
    /// will eventually need. Port of `self.all_images`.
    pub fn all_images(&self) -> &std::collections::HashSet<String> {
        &self.all_images
    }

    /// Port of `Images.unique_name`.
    fn unique_name(&self, base: &str) -> String {
        let exists: std::collections::HashSet<&String> = self.used.values().collect();
        let mut c = 1;
        let mut name = base.to_string();
        while exists.contains(&name) {
            let (n, e) = match base.rsplit_once('.') {
                Some((n, e)) => (n.to_string(), e.to_string()),
                None => (String::new(), base.to_string()),
            };
            name = format!("{n}-{c}.{e}");
            c += 1;
        }
        name
    }

    /// Resolves `rid` against `rid_map` (a resolved zip-path map, e.g.
    /// `Relationships::by_id` -- pass whichever document's/footnote's
    /// own relationships apply at the call site, since Python's
    /// `self.rid_map` swap-and-restore dance for footnotes becomes an
    /// explicit parameter here instead), reads and (optionally)
    /// resizes its image data, writes it under a unique name inside
    /// `images_dir` (already `dest_dir/images` -- see the module docs
    /// on `to_html`, not yet ported, for why this takes the
    /// *already-suffixed* directory rather than `dest_dir` itself),
    /// and returns that filename (not a full path).
    ///
    /// Two dedup layers, matching Python exactly: same `(fname,
    /// max_size)` returns the same name without re-reading anything;
    /// and if resizing turned out to be a no-op (image already small
    /// enough), the *unsized* cache entry is checked/populated too, so
    /// a later unsized request for the same image reuses the file
    /// already on disk instead of writing a byte-identical duplicate.
    ///
    /// Port of `Images.generate_filename`.
    pub fn generate_filename<R: Read + Seek>(
        &mut self,
        docx: &mut Docx<R>,
        images_dir: &Path,
        rid: &str,
        base: Option<&str>,
        rid_map: &HashMap<String, String>,
        max_size: Option<(u32, u32)>,
    ) -> Result<String, LinkedImageNotFound> {
        let fname = rid_map
            .get(rid)
            .cloned()
            .ok_or_else(|| LinkedImageNotFound {
                fname: rid.to_string(),
            })?;
        let key = (fname.clone(), max_size);
        if let Some(name) = self.used.get(&key) {
            return Ok(name.clone());
        }

        let (mut raw, mut computed_base) = read_image_data(docx, &fname, base)?;
        let mut resized = false;
        if let Some((max_width, max_height)) = max_size {
            let (r, b, did_resize) = resize_image(&raw, &computed_base, max_width, max_height);
            raw = r;
            computed_base = b;
            resized = did_resize;
        }
        let name = self.unique_name(&computed_base);
        self.used.insert(key, name.clone());

        if max_size.is_some() && !resized {
            let okey = (fname, None);
            if let Some(existing) = self.used.get(&okey) {
                return Ok(existing.clone());
            }
            self.used.insert(okey, name.clone());
        }

        let _ = std::fs::create_dir_all(images_dir);
        std::fs::write(images_dir.join(&name), &raw).map_err(|_| LinkedImageNotFound {
            fname: name.clone(),
        })?;
        self.all_images.insert(format!("images/{name}"));
        Ok(name)
    }

    /// Builds an `<img>` from `pic`'s embedded/linked blip (`a:blip`,
    /// under a `pic:cNvPr` naming/alt-text carrier -- multiple of
    /// either yields the *last* one, matching Python's unbroken
    /// reassigning loops), returning `None` if `pic` has no readable
    /// image at all. `name`/`alt` come from `pic:cNvPr`'s `name`/
    /// `descr`; `title` is `get_image_properties`'s `wp:docPr` title,
    /// *overridden* by a wrapping `a:hlinkClick`'s own `tooltip` if
    /// present (a real behavior, not a bug: Python's `pic_to_img`
    /// reuses its own `title` parameter name for this, so the
    /// hyperlink's tooltip silently wins over the docPr title when
    /// both exist). A blip's own `a:extLst` SVG alternative is
    /// preferred over its raster embed/link when its relationship id
    /// is known.
    ///
    /// Port of `Images.pic_to_img`.
    #[allow(clippy::too_many_arguments)]
    pub fn pic_to_img<'a, 'i, R: Read + Seek>(
        &mut self,
        dom: &mut Dom,
        docx: &mut Docx<R>,
        images_dir: &Path,
        ns: &DocxNamespace,
        pic: Node<'a, 'i>,
        alt: Option<&str>,
        parent: Node<'a, 'i>,
        title: Option<&str>,
        rid_map: &HashMap<String, String>,
    ) -> Option<NodeId> {
        let mut link: Option<ImageLink> = None;
        let mut title = title.map(str::to_string);
        for hl in ns.descendants(parent, &["a:hlinkClick"]) {
            let Some(id) = ns.get(hl, "r:id") else {
                continue;
            };
            let mut l = ImageLink {
                id: id.to_string(),
                target: None,
                title: None,
            };
            if let Some(tgt) = ns.get(hl, "tgtFrame") {
                l.target = Some(tgt.to_string());
            }
            if let Some(tt) = ns.get(hl, "tooltip") {
                title = Some(tt.to_string());
                l.title = Some(tt.to_string());
            }
            link = Some(l);
        }

        let mut alt = alt.map(str::to_string);
        for pr in ns.descendants(pic, &["pic:cNvPr"]) {
            let name = ns
                .get(pr, "name")
                .filter(|n| !n.is_empty())
                .map(image_filename);
            if let Some(descr) = ns.get(pr, "descr").filter(|s| !s.is_empty()) {
                alt = Some(descr.to_string());
            }

            for a in ns.descendants(pic, &["a:blip"]) {
                let mut rid = ns
                    .get(a, "r:embed")
                    .or_else(|| ns.get(a, "r:link"))
                    .map(str::to_string);

                'ext: for ext_lst in ns.children(a, &["a:extLst"]) {
                    for ext in ns.children(ext_lst, &["a:ext"]) {
                        if ns.get(ext, "uri") != Some(SVG_BLIP_URI) {
                            continue;
                        }
                        for asvg in ns.children(ext, &["asvg:svgBlip"]) {
                            let svg_rid =
                                ns.get(asvg, "r:embed").or_else(|| ns.get(asvg, "r:link"));
                            if let Some(svg_rid) = svg_rid {
                                if rid_map.contains_key(svg_rid) {
                                    rid = Some(svg_rid.to_string());
                                    break 'ext;
                                }
                            }
                        }
                    }
                }

                let Some(rid) = rid else { continue };
                if !rid_map.contains_key(&rid) {
                    continue;
                }

                let src = match self.generate_filename(
                    docx,
                    images_dir,
                    &rid,
                    name.as_deref(),
                    rid_map,
                    None,
                ) {
                    Ok(s) => s,
                    Err(_) => continue,
                };

                let img = dom.new_element("img");
                let alt_val = alt
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "Image".to_string());
                dom.node_mut(img)
                    .attrs
                    .insert("src".to_string(), format!("images/{src}"));
                dom.node_mut(img).attrs.insert("alt".to_string(), alt_val);
                if let Some(t) = title.as_deref().filter(|t| !t.is_empty()) {
                    dom.node_mut(img)
                        .attrs
                        .insert("title".to_string(), t.to_string());
                }
                if let Some(l) = link {
                    self.links.push((img, l, rid_map.clone()));
                }
                return Some(img);
            }
        }
        None
    }

    /// Builds one `<img>` per inline/floated picture in `drawing`
    /// (`wp:inline`/`wp:anchor`), setting each one's CSS from
    /// [`get_image_properties`] (and, for floats, [`get_float_properties`]
    /// too) as an inline `style` attribute.
    ///
    /// Port of `Images.drawing_to_html`.
    pub fn drawing_to_html<'a, 'i, R: Read + Seek>(
        &mut self,
        dom: &mut Dom,
        docx: &mut Docx<R>,
        images_dir: &Path,
        ns: &DocxNamespace,
        drawing: Node<'a, 'i>,
        page: &PageProperties,
        rid_map: &HashMap<String, String>,
    ) -> Vec<NodeId> {
        let mut out = Vec::new();

        for inline in ns.children(drawing, &["wp:inline"]) {
            let (style, alt, title) = get_image_properties(inline, ns);
            for pic in ns.descendants(inline, &["pic:pic"]) {
                if let Some(img) = self.pic_to_img(
                    dom,
                    docx,
                    images_dir,
                    ns,
                    pic,
                    alt.as_deref(),
                    inline,
                    title.as_deref(),
                    rid_map,
                ) {
                    if !style.is_empty() {
                        dom.node_mut(img)
                            .attrs
                            .insert("style".to_string(), css_to_style_string(&style));
                    }
                    out.push(img);
                }
            }
        }

        for anchor in ns.children(drawing, &["wp:anchor"]) {
            let (mut style, alt, title) = get_image_properties(anchor, ns);
            get_float_properties(anchor, &mut style, page, ns);
            for pic in ns.descendants(anchor, &["pic:pic"]) {
                if let Some(img) = self.pic_to_img(
                    dom,
                    docx,
                    images_dir,
                    ns,
                    pic,
                    alt.as_deref(),
                    anchor,
                    title.as_deref(),
                    rid_map,
                ) {
                    if !style.is_empty() {
                        dom.node_mut(img)
                            .attrs
                            .insert("style".to_string(), css_to_style_string(&style));
                    }
                    out.push(img);
                }
            }
        }

        out
    }

    /// Handles VML (`w:pict`) markup: a `<hr>` if `pict`'s sole element
    /// child is a Word "insert horizontal line" shape (`o:hr`), styled
    /// from `o:hrpct`/`o:hralign`; plus one `<img>` per `v:imagedata`
    /// with a known relationship id, `display: block`-styled if its
    /// parent `v:shape`'s own `style` attribute says
    /// `position:absolute` (VML positions shapes via inline CSS the
    /// same way `wp:anchor` uses DrawingML attributes).
    ///
    /// Port of `Images.pict_to_html`. A parent `v:shape` with no
    /// `style` attribute at all is treated as simply not
    /// `position:absolute` rather than reproducing Python's
    /// `'position:absolute' in style` crash on `style` being `None`
    /// (`get(elem, 'style')` with no default) -- nothing useful to
    /// reproduce about a crash on malformed/unusual VML.
    pub fn pict_to_html<R: Read + Seek>(
        &mut self,
        dom: &mut Dom,
        docx: &mut Docx<R>,
        images_dir: &Path,
        ns: &DocxNamespace,
        pict: Node,
        rid_map: &HashMap<String, String>,
    ) -> Vec<NodeId> {
        let mut out = Vec::new();

        let first_child = pict.children().find(|c| c.is_element());
        let is_hr = first_child
            .map(|c| matches!(ns.get(c, "o:hr"), Some("t") | Some("true")))
            .unwrap_or(false);
        if let (true, Some(first_child)) = (is_hr, first_child) {
            let mut style = Css::new();
            if let Some(pct) = ns
                .get(first_child, "o:hrpct")
                .and_then(|s| s.parse::<f64>().ok())
            {
                if pct > 0.0 {
                    style.insert("width".to_string(), format!("{}%", format_g(pct, 3)));
                }
            }
            let align = ns.get_or(first_child, "o:hralign", "center");
            if align == "left" || align == "right" {
                let (ml, mr) = if align == "left" {
                    ("0", "auto")
                } else {
                    ("auto", "0")
                };
                style.insert("margin-left".to_string(), ml.to_string());
                style.insert("margin-right".to_string(), mr.to_string());
            }
            let hr = dom.new_element("hr");
            if !style.is_empty() {
                let style_str = style
                    .iter()
                    .map(|(k, v)| format!("{k}:{v}"))
                    .collect::<Vec<_>>()
                    .join("; ");
                dom.node_mut(hr)
                    .attrs
                    .insert("style".to_string(), style_str);
            }
            out.push(hr);
        }

        for imagedata in ns.descendants(pict, &["v:imagedata"]) {
            let Some(rid) = ns.get(imagedata, "r:id") else {
                continue;
            };
            if !rid_map.contains_key(rid) {
                continue;
            }
            let src = match self.generate_filename(docx, images_dir, rid, None, rid_map, None) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let img = dom.new_element("img");
            dom.node_mut(img)
                .attrs
                .insert("src".to_string(), format!("images/{src}"));
            let alt = ns
                .get(imagedata, "o:title")
                .filter(|s| !s.is_empty())
                .unwrap_or("Image");
            dom.node_mut(img)
                .attrs
                .insert("alt".to_string(), alt.to_string());
            if let Some(parent) = imagedata.parent() {
                if let Some(pstyle) = ns.get(parent, "style") {
                    if pstyle.contains("position:absolute") {
                        dom.node_mut(img)
                            .attrs
                            .insert("style".to_string(), "display: block".to_string());
                    }
                }
            }
            out.push(img);
        }

        out
    }

    /// `drawing_to_html` for a `w:drawing` element, `pict_to_html` for
    /// anything else (`w:pict`) -- creates `dest_dir/images/` if
    /// needed first.
    ///
    /// Port of `Images.to_html`.
    pub fn to_html<R: Read + Seek>(
        &mut self,
        dom: &mut Dom,
        docx: &mut Docx<R>,
        dest_dir: &Path,
        ns: &DocxNamespace,
        elem: Node,
        page: &PageProperties,
        rid_map: &HashMap<String, String>,
    ) -> Vec<NodeId> {
        let images_dir = dest_dir.join("images");
        let _ = std::fs::create_dir_all(&images_dir);
        if ns.is_tag(elem, "w:drawing") {
            self.drawing_to_html(dom, docx, &images_dir, ns, elem, page, rid_map)
        } else {
            self.pict_to_html(dom, docx, &images_dir, ns, elem, rid_map)
        }
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

    mod images_tests {
        use super::*;
        use std::io::{Cursor, Write};

        fn png_bytes(width: u32, height: u32) -> Vec<u8> {
            let img = image::DynamicImage::new_rgb8(width, height);
            let mut buf = Cursor::new(Vec::new());
            img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
            buf.into_inner()
        }

        fn package(parts: &[(&str, &[u8])]) -> Docx<Cursor<Vec<u8>>> {
            let mut buf = Vec::new();
            {
                let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
                let options = zip::write::FileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored);
                for (name, content) in parts {
                    zip.start_file(*name, options).unwrap();
                    zip.write_all(content).unwrap();
                }
                zip.finish().unwrap();
            }
            Docx::new(Cursor::new(buf)).unwrap()
        }

        const CONTENT_TYPES: &[u8] = br#"<?xml version="1.0"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="png" ContentType="image/png"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#;

        const RELS: &[u8] = br#"<?xml version="1.0"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;

        fn docx_with_image(name: &str, data: &[u8]) -> Docx<Cursor<Vec<u8>>> {
            package(&[
                ("[Content_Types].xml", CONTENT_TYPES),
                ("_rels/.rels", RELS),
                ("word/document.xml", b"<w:document/>"),
                (name, data),
            ])
        }

        #[test]
        fn generate_filename_reads_writes_and_caches_an_embedded_image() {
            let dir = tempfile::tempdir().unwrap();
            let images_dir = dir.path().join("images");
            let mut docx = docx_with_image("word/media/image1.png", &png_bytes(50, 50));
            let rid_map =
                HashMap::from([("rId4".to_string(), "word/media/image1.png".to_string())]);
            let mut images = Images::new();

            let name1 = images
                .generate_filename(&mut docx, &images_dir, "rId4", None, &rid_map, None)
                .unwrap();
            assert!(images_dir.join(&name1).exists());
            assert!(images.all_images().contains(&format!("images/{name1}")));

            // Same rid + size key -> cached, no re-read/re-write needed.
            let name2 = images
                .generate_filename(&mut docx, &images_dir, "rId4", None, &rid_map, None)
                .unwrap();
            assert_eq!(name1, name2);
        }

        #[test]
        fn an_unknown_rid_is_reported() {
            let dir = tempfile::tempdir().unwrap();
            let mut docx = docx_with_image("word/media/image1.png", &png_bytes(10, 10));
            let mut images = Images::new();
            let err = images
                .generate_filename(
                    &mut docx,
                    &dir.path().join("images"),
                    "rIdMissing",
                    None,
                    &HashMap::new(),
                    None,
                )
                .unwrap_err();
            assert_eq!(err.fname, "rIdMissing");
        }

        #[test]
        fn a_linked_file_url_is_read_from_disk_not_the_zip() {
            let dir = tempfile::tempdir().unwrap();
            let linked = dir.path().join("external.png");
            std::fs::write(&linked, png_bytes(20, 20)).unwrap();
            let mut docx = docx_with_image("word/media/unused.png", &png_bytes(5, 5));
            let rid_map =
                HashMap::from([("rId9".to_string(), format!("file://{}", linked.display()))]);
            let mut images = Images::new();

            let images_dir = dir.path().join("images");
            let name = images
                .generate_filename(&mut docx, &images_dir, "rId9", None, &rid_map, None)
                .unwrap();
            assert!(images_dir.join(&name).exists());
        }

        #[test]
        fn a_missing_linked_file_is_reported() {
            let dir = tempfile::tempdir().unwrap();
            let mut docx = docx_with_image("word/media/unused.png", &png_bytes(5, 5));
            let rid_map =
                HashMap::from([("rId9".to_string(), "file:///no/such/path.png".to_string())]);
            let mut images = Images::new();

            let err = images
                .generate_filename(
                    &mut docx,
                    &dir.path().join("images"),
                    "rId9",
                    None,
                    &rid_map,
                    None,
                )
                .unwrap_err();
            assert_eq!(err.fname, "/no/such/path.png");
        }

        #[test]
        fn resize_image_shrinks_an_oversized_image_and_suffixes_the_name() {
            let raw = png_bytes(1000, 1000);
            let (resized_raw, new_base, resized) = resize_image(&raw, "image.png", 100, 100);
            assert!(resized);
            assert_eq!(new_base, "image-100x100.png");
            let img = image::load_from_memory(&resized_raw).unwrap();
            assert_eq!(img.dimensions(), (100, 100));
        }

        #[test]
        fn resize_image_leaves_a_small_image_alone() {
            let raw = png_bytes(50, 50);
            let (resized_raw, new_base, resized) = resize_image(&raw, "image.png", 100, 100);
            assert!(!resized);
            assert_eq!(new_base, "image.png");
            assert_eq!(resized_raw, raw);
        }

        #[test]
        fn generate_filename_with_a_size_limit_resizes_and_writes_a_suffixed_file() {
            let dir = tempfile::tempdir().unwrap();
            let images_dir = dir.path().join("images");
            let mut docx = docx_with_image("word/media/image1.png", &png_bytes(1000, 1000));
            let rid_map =
                HashMap::from([("rId4".to_string(), "word/media/image1.png".to_string())]);
            let mut images = Images::new();

            let name = images
                .generate_filename(
                    &mut docx,
                    &images_dir,
                    "rId4",
                    None,
                    &rid_map,
                    Some((20, 20)),
                )
                .unwrap();
            assert!(name.contains("20x20"));
            assert!(images_dir.join(&name).exists());
        }

        #[test]
        fn two_different_images_sharing_a_base_name_get_distinct_unique_names() {
            let dir = tempfile::tempdir().unwrap();
            let images_dir = dir.path().join("images");
            let mut docx = package(&[
                ("[Content_Types].xml", CONTENT_TYPES),
                ("_rels/.rels", RELS),
                ("word/document.xml", b"<w:document/>"),
                ("word/media/image1.png", &png_bytes(10, 10)),
                ("word/media/image2.png", &png_bytes(20, 20)),
            ]);
            let rid_map = HashMap::from([
                ("rId4".to_string(), "word/media/image1.png".to_string()),
                ("rId5".to_string(), "word/media/image2.png".to_string()),
            ]);
            let mut images = Images::new();

            // An explicit `base` (e.g. from a `pic:cNvPr` name attribute)
            // forces both distinct images toward the same requested
            // filename, which `unique_name` must disambiguate.
            let name1 = images
                .generate_filename(
                    &mut docx,
                    &images_dir,
                    "rId4",
                    Some("picture.png"),
                    &rid_map,
                    None,
                )
                .unwrap();
            let name2 = images
                .generate_filename(
                    &mut docx,
                    &images_dir,
                    "rId5",
                    Some("picture.png"),
                    &rid_map,
                    None,
                )
                .unwrap();
            assert_ne!(
                name1, name2,
                "both requested picture.png -> the second must be de-duplicated"
            );
            assert!(images_dir.join(&name1).exists());
            assert!(images_dir.join(&name2).exists());
        }

        mod markup_tests {
            use super::*;

            const DOC_OPEN: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office""#;

            fn parse(body: &str) -> (roxmltree::Document<'static>, DocxNamespace) {
                let xml: &'static str =
                    Box::leak(format!("<root {DOC_OPEN}>{body}</root>").into_boxed_str());
                (
                    roxmltree::Document::parse(xml).expect("valid XML"),
                    DocxNamespace::default(),
                )
            }

            fn pic_fragment(embed_rid: &str, name: &str) -> String {
                format!(
                    r#"<pic:pic>
                     <pic:nvPicPr><pic:cNvPr id="1" name="{name}"/></pic:nvPicPr>
                     <pic:blipFill><a:blip r:embed="{embed_rid}"/></pic:blipFill>
                   </pic:pic>"#
                )
            }

            mod pic_to_img_tests {
                use super::*;

                #[test]
                fn a_pic_with_a_known_embed_becomes_an_img() {
                    let (doc, ns) = parse(&pic_fragment("rId4", "pic1.png"));
                    let pic = ns
                        .descendants(doc.root_element(), &["pic:pic"])
                        .into_iter()
                        .next()
                        .unwrap();
                    let dir = tempfile::tempdir().unwrap();
                    let images_dir = dir.path().join("images");
                    let mut docx = docx_with_image("word/media/image1.png", &png_bytes(10, 10));
                    let rid_map =
                        HashMap::from([("rId4".to_string(), "word/media/image1.png".to_string())]);
                    let mut images = Images::new();
                    let mut dom = Dom::empty();

                    let img = images
                        .pic_to_img(
                            &mut dom,
                            &mut docx,
                            &images_dir,
                            &ns,
                            pic,
                            Some("fallback alt"),
                            pic,
                            None,
                            &rid_map,
                        )
                        .unwrap();

                    assert_eq!(dom.tag(img), Some("img"));
                    assert!(dom
                        .node(img)
                        .attrs
                        .get("src")
                        .unwrap()
                        .starts_with("images/"));
                    assert_eq!(
                        dom.node(img).attrs.get("alt").map(String::as_str),
                        Some("fallback alt")
                    );
                }

                #[test]
                fn cnvpr_descr_overrides_the_passed_in_alt() {
                    let (doc, ns) = parse(
                        r#"<pic:pic>
                         <pic:nvPicPr><pic:cNvPr id="1" name="pic1.png" descr="A cat"/></pic:nvPicPr>
                         <pic:blipFill><a:blip r:embed="rId4"/></pic:blipFill>
                       </pic:pic>"#,
                    );
                    let pic = ns
                        .descendants(doc.root_element(), &["pic:pic"])
                        .into_iter()
                        .next()
                        .unwrap();
                    let dir = tempfile::tempdir().unwrap();
                    let images_dir = dir.path().join("images");
                    let mut docx = docx_with_image("word/media/image1.png", &png_bytes(10, 10));
                    let rid_map =
                        HashMap::from([("rId4".to_string(), "word/media/image1.png".to_string())]);
                    let mut images = Images::new();
                    let mut dom = Dom::empty();

                    let img = images
                        .pic_to_img(
                            &mut dom,
                            &mut docx,
                            &images_dir,
                            &ns,
                            pic,
                            Some("fallback alt"),
                            pic,
                            None,
                            &rid_map,
                        )
                        .unwrap();

                    assert_eq!(
                        dom.node(img).attrs.get("alt").map(String::as_str),
                        Some("A cat")
                    );
                }

                #[test]
                fn no_matching_blip_returns_none() {
                    let (doc, ns) = parse(
                        r#"<pic:pic><pic:nvPicPr><pic:cNvPr id="1" name="x"/></pic:nvPicPr></pic:pic>"#,
                    );
                    let pic = ns
                        .descendants(doc.root_element(), &["pic:pic"])
                        .into_iter()
                        .next()
                        .unwrap();
                    let dir = tempfile::tempdir().unwrap();
                    let mut docx = docx_with_image("word/media/image1.png", &png_bytes(10, 10));
                    let mut images = Images::new();
                    let mut dom = Dom::empty();

                    let img = images.pic_to_img(
                        &mut dom,
                        &mut docx,
                        &dir.path().join("images"),
                        &ns,
                        pic,
                        None,
                        pic,
                        None,
                        &HashMap::new(),
                    );
                    assert!(img.is_none());
                }

                #[test]
                fn an_unknown_rid_is_skipped_and_returns_none() {
                    let (doc, ns) = parse(&pic_fragment("rIdMissing", "pic1.png"));
                    let pic = ns
                        .descendants(doc.root_element(), &["pic:pic"])
                        .into_iter()
                        .next()
                        .unwrap();
                    let dir = tempfile::tempdir().unwrap();
                    let mut docx = docx_with_image("word/media/image1.png", &png_bytes(10, 10));
                    let rid_map =
                        HashMap::from([("rId4".to_string(), "word/media/image1.png".to_string())]);
                    let mut images = Images::new();
                    let mut dom = Dom::empty();

                    let img = images.pic_to_img(
                        &mut dom,
                        &mut docx,
                        &dir.path().join("images"),
                        &ns,
                        pic,
                        None,
                        pic,
                        None,
                        &rid_map,
                    );
                    assert!(img.is_none());
                }

                #[test]
                fn a_hyperlinked_picture_records_a_link_and_its_tooltip_overrides_the_title() {
                    let (doc, ns) = parse(&format!(
                        r#"<wp:inline>
                         <a:hlinkClick r:id="rId8" tgtFrame="_blank" tooltip="See more"/>
                         {}
                       </wp:inline>"#,
                        pic_fragment("rId4", "pic1.png")
                    ));
                    let parent = ns
                        .descendants(doc.root_element(), &["wp:inline"])
                        .into_iter()
                        .next()
                        .unwrap();
                    let pic = ns
                        .descendants(parent, &["pic:pic"])
                        .into_iter()
                        .next()
                        .unwrap();
                    let dir = tempfile::tempdir().unwrap();
                    let images_dir = dir.path().join("images");
                    let mut docx = docx_with_image("word/media/image1.png", &png_bytes(10, 10));
                    let rid_map =
                        HashMap::from([("rId4".to_string(), "word/media/image1.png".to_string())]);
                    let mut images = Images::new();
                    let mut dom = Dom::empty();

                    let img = images
                        .pic_to_img(
                            &mut dom,
                            &mut docx,
                            &images_dir,
                            &ns,
                            pic,
                            None,
                            parent,
                            Some("docPr title"),
                            &rid_map,
                        )
                        .unwrap();

                    assert_eq!(images.links.len(), 1);
                    let (linked_img, link, _) = &images.links[0];
                    assert_eq!(*linked_img, img);
                    assert_eq!(link.id, "rId8");
                    assert_eq!(link.target.as_deref(), Some("_blank"));
                    assert_eq!(link.title.as_deref(), Some("See more"));
                    assert_eq!(
                        dom.node(img).attrs.get("title").map(String::as_str),
                        Some("See more")
                    );
                }
            }

            mod drawing_to_html_tests {
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
                fn an_inline_picture_gets_its_extent_as_a_style() {
                    let (doc, ns) = parse(&format!(
                        r#"<w:drawing>
                         <wp:inline>
                           <wp:extent cx="914400" cy="457200"/>
                           {}
                         </wp:inline>
                       </w:drawing>"#,
                        pic_fragment("rId4", "pic1.png")
                    ));
                    let drawing = ns
                        .descendants(doc.root_element(), &["w:drawing"])
                        .into_iter()
                        .next()
                        .unwrap();
                    let dir = tempfile::tempdir().unwrap();
                    let images_dir = dir.path().join("images");
                    let mut docx = docx_with_image("word/media/image1.png", &png_bytes(10, 10));
                    let rid_map =
                        HashMap::from([("rId4".to_string(), "word/media/image1.png".to_string())]);
                    let mut images = Images::new();
                    let mut dom = Dom::empty();

                    let imgs = images.drawing_to_html(
                        &mut dom,
                        &mut docx,
                        &images_dir,
                        &ns,
                        drawing,
                        &page(),
                        &rid_map,
                    );

                    assert_eq!(imgs.len(), 1);
                    assert_eq!(
                        dom.node(imgs[0]).attrs.get("style").map(String::as_str),
                        Some("width: 72pt; height: 36pt")
                    );
                }

                #[test]
                fn a_floated_anchor_picture_gets_float_properties_applied() {
                    let (doc, ns) = parse(&format!(
                        r#"<w:drawing>
                         <wp:anchor>
                           <wp:positionH relativeFrom="page"><wp:align>left</wp:align></wp:positionH>
                           <wp:wrapSquare wrapText="bothSides"/>
                           {}
                         </wp:anchor>
                       </w:drawing>"#,
                        pic_fragment("rId4", "pic1.png")
                    ));
                    let drawing = ns
                        .descendants(doc.root_element(), &["w:drawing"])
                        .into_iter()
                        .next()
                        .unwrap();
                    let dir = tempfile::tempdir().unwrap();
                    let images_dir = dir.path().join("images");
                    let mut docx = docx_with_image("word/media/image1.png", &png_bytes(10, 10));
                    let rid_map =
                        HashMap::from([("rId4".to_string(), "word/media/image1.png".to_string())]);
                    let mut images = Images::new();
                    let mut dom = Dom::empty();

                    let imgs = images.drawing_to_html(
                        &mut dom,
                        &mut docx,
                        &images_dir,
                        &ns,
                        drawing,
                        &page(),
                        &rid_map,
                    );

                    assert_eq!(imgs.len(), 1);
                    let style = dom
                        .node(imgs[0])
                        .attrs
                        .get("style")
                        .cloned()
                        .unwrap_or_default();
                    assert!(style.contains("float: left"), "style was: {style}");
                }

                #[test]
                fn no_pictures_yields_no_images() {
                    let (doc, ns) = parse("<w:drawing><wp:inline/></w:drawing>");
                    let drawing = ns
                        .descendants(doc.root_element(), &["w:drawing"])
                        .into_iter()
                        .next()
                        .unwrap();
                    let dir = tempfile::tempdir().unwrap();
                    let mut docx = docx_with_image("word/media/image1.png", &png_bytes(10, 10));
                    let mut images = Images::new();
                    let mut dom = Dom::empty();

                    let imgs = images.drawing_to_html(
                        &mut dom,
                        &mut docx,
                        &dir.path().join("images"),
                        &ns,
                        drawing,
                        &page(),
                        &HashMap::new(),
                    );
                    assert!(imgs.is_empty());
                }
            }

            mod pict_to_html_tests {
                use super::*;

                #[test]
                fn a_horizontal_rule_shape_becomes_an_hr() {
                    let (doc, ns) = parse(
                        r#"<w:pict><v:rect o:hr="t" o:hrpct="50" o:hralign="left"/></w:pict>"#,
                    );
                    let pict = ns
                        .descendants(doc.root_element(), &["w:pict"])
                        .into_iter()
                        .next()
                        .unwrap();
                    let dir = tempfile::tempdir().unwrap();
                    let mut docx = docx_with_image("word/media/image1.png", &png_bytes(10, 10));
                    let mut images = Images::new();
                    let mut dom = Dom::empty();

                    let out = images.pict_to_html(
                        &mut dom,
                        &mut docx,
                        &dir.path().join("images"),
                        &ns,
                        pict,
                        &HashMap::new(),
                    );

                    assert_eq!(out.len(), 1);
                    assert_eq!(dom.tag(out[0]), Some("hr"));
                    let style = dom
                        .node(out[0])
                        .attrs
                        .get("style")
                        .cloned()
                        .unwrap_or_default();
                    assert!(style.contains("width:50%"), "style was: {style}");
                    assert!(style.contains("margin-left:0"), "style was: {style}");
                }

                #[test]
                fn v_imagedata_becomes_an_img() {
                    let (doc, ns) = parse(
                        r#"<w:pict><v:shape><v:imagedata r:id="rId4" o:title="A cat"/></v:shape></w:pict>"#,
                    );
                    let pict = ns
                        .descendants(doc.root_element(), &["w:pict"])
                        .into_iter()
                        .next()
                        .unwrap();
                    let dir = tempfile::tempdir().unwrap();
                    let images_dir = dir.path().join("images");
                    let mut docx = docx_with_image("word/media/image1.png", &png_bytes(10, 10));
                    let rid_map =
                        HashMap::from([("rId4".to_string(), "word/media/image1.png".to_string())]);
                    let mut images = Images::new();
                    let mut dom = Dom::empty();

                    let out =
                        images.pict_to_html(&mut dom, &mut docx, &images_dir, &ns, pict, &rid_map);

                    assert_eq!(out.len(), 1);
                    assert_eq!(dom.tag(out[0]), Some("img"));
                    assert_eq!(
                        dom.node(out[0]).attrs.get("alt").map(String::as_str),
                        Some("A cat")
                    );
                }

                #[test]
                fn an_absolutely_positioned_shapes_image_gets_display_block() {
                    let (doc, ns) = parse(
                        r#"<w:pict><v:shape style="position:absolute;left:0"><v:imagedata r:id="rId4"/></v:shape></w:pict>"#,
                    );
                    let pict = ns
                        .descendants(doc.root_element(), &["w:pict"])
                        .into_iter()
                        .next()
                        .unwrap();
                    let dir = tempfile::tempdir().unwrap();
                    let images_dir = dir.path().join("images");
                    let mut docx = docx_with_image("word/media/image1.png", &png_bytes(10, 10));
                    let rid_map =
                        HashMap::from([("rId4".to_string(), "word/media/image1.png".to_string())]);
                    let mut images = Images::new();
                    let mut dom = Dom::empty();

                    let out =
                        images.pict_to_html(&mut dom, &mut docx, &images_dir, &ns, pict, &rid_map);

                    assert_eq!(
                        dom.node(out[0]).attrs.get("style").map(String::as_str),
                        Some("display: block")
                    );
                }

                #[test]
                fn an_unknown_rid_is_skipped() {
                    let (doc, ns) = parse(
                        r#"<w:pict><v:shape><v:imagedata r:id="rIdMissing"/></v:shape></w:pict>"#,
                    );
                    let pict = ns
                        .descendants(doc.root_element(), &["w:pict"])
                        .into_iter()
                        .next()
                        .unwrap();
                    let dir = tempfile::tempdir().unwrap();
                    let mut docx = docx_with_image("word/media/image1.png", &png_bytes(10, 10));
                    let mut images = Images::new();
                    let mut dom = Dom::empty();

                    let out = images.pict_to_html(
                        &mut dom,
                        &mut docx,
                        &dir.path().join("images"),
                        &ns,
                        pict,
                        &HashMap::new(),
                    );
                    assert!(out.is_empty());
                }
            }

            mod to_html_tests {
                use super::*;

                fn page() -> PageProperties {
                    PageProperties::default()
                }

                #[test]
                fn a_w_drawing_element_dispatches_to_drawing_to_html_and_creates_the_images_dir() {
                    let (doc, ns) = parse(&format!(
                        r#"<w:drawing><wp:inline>{}</wp:inline></w:drawing>"#,
                        pic_fragment("rId4", "pic1.png")
                    ));
                    let drawing = ns
                        .descendants(doc.root_element(), &["w:drawing"])
                        .into_iter()
                        .next()
                        .unwrap();
                    let dir = tempfile::tempdir().unwrap();
                    let mut docx = docx_with_image("word/media/image1.png", &png_bytes(10, 10));
                    let rid_map =
                        HashMap::from([("rId4".to_string(), "word/media/image1.png".to_string())]);
                    let mut images = Images::new();
                    let mut dom = Dom::empty();

                    let out = images.to_html(
                        &mut dom,
                        &mut docx,
                        dir.path(),
                        &ns,
                        drawing,
                        &page(),
                        &rid_map,
                    );

                    assert_eq!(out.len(), 1);
                    assert!(dir.path().join("images").is_dir());
                }

                #[test]
                fn a_w_pict_element_dispatches_to_pict_to_html() {
                    let (doc, ns) = parse(r#"<w:pict><v:rect o:hr="t"/></w:pict>"#);
                    let pict = ns
                        .descendants(doc.root_element(), &["w:pict"])
                        .into_iter()
                        .next()
                        .unwrap();
                    let dir = tempfile::tempdir().unwrap();
                    let mut docx = docx_with_image("word/media/image1.png", &png_bytes(10, 10));
                    let mut images = Images::new();
                    let mut dom = Dom::empty();

                    let out = images.to_html(
                        &mut dom,
                        &mut docx,
                        dir.path(),
                        &ns,
                        pict,
                        &page(),
                        &HashMap::new(),
                    );

                    assert_eq!(out.len(), 1);
                    assert_eq!(dom.tag(out[0]), Some("hr"));
                }
            }
        }
    }
}
