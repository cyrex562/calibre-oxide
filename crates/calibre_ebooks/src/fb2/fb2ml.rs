//! XHTML → FictionBook 2 markup.
//!
//! Port of `old_src/src/calibre/ebooks/fb2/fb2ml.py`.
//!
//! FB2 is one XML document: a `<description>` of metadata, a `<body>`
//! of `<section>`s and `<p>`s, and the images base64'd into `<binary>`
//! elements at the end. The conversion walks each spine item's XHTML
//! and emits markup, then cleans the result up with a pass of regular
//! expressions — which is how calibre does it too, and why
//! [`clean_text`] is ported literally rather than reimagined.
//!
//! # Where the stylizer went
//!
//! The Python takes a `Stylizer` and asks it for six properties per
//! element: `display`, `visibility`, `font-weight`, `font-style`,
//! `text-decoration`, and the margin/font-size pair it turns into a
//! count of blank lines. `calibre_ebooks`'s own stylizer is still a
//! stub with no cascade, so rather than port against something that
//! would invent answers, this module takes a [`Stylizer`] trait
//! supplying exactly those six. [`TagStylizer`] implements it from the
//! HTML defaults alone, which is enough for documents whose styling is
//! in their markup; a real cascade can be dropped in later without
//! touching this file.

use std::collections::HashMap;

use base64::Engine;
use regex::Regex;
use roxmltree::{Document, Node};

use crate::oeb::book::OEBBook;
use crate::oeb::toc::TOCNode;
use crate::xml_util::prepare_string_for_xml;

/// The XHTML namespace, which content must be in to be converted.
pub const XHTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// Media types FB2 can carry directly in a `<binary>`.
const NATIVE_IMAGE_TYPES: [&str; 2] = ["image/jpeg", "image/png"];

/// Raster image types calibre is willing to embed, converting the ones
/// FB2 does not take natively.
const RASTER_IMAGE_TYPES: [&str; 6] = [
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/bmp",
    "image/tiff",
    "image/webp",
];

/// How the converter decides where `<section>`s begin.
///
/// Port of the `sectionize` conversion option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sectionize {
    /// One section wrapping the whole book.
    #[default]
    Nothing,
    /// A section per spine item.
    Files,
    /// A section per table-of-contents entry, with `<title>`s.
    Toc,
}

/// The conversion options this file reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Fb2Options {
    pub sectionize: Sectionize,
    /// Emit an `<empty-line/>` after every paragraph.
    pub insert_blank_line: bool,
    /// The `<genre>` written into the description.
    pub genre: String,
    /// Program name and version for `<program-used>`.
    pub program: String,
}

impl Default for Fb2Options {
    fn default() -> Self {
        Self {
            sectionize: Sectionize::default(),
            insert_blank_line: false,
            // calibre's default for --fb2-genre.
            genre: "antique".to_string(),
            program: format!("calibre-oxide {}", env!("CARGO_PKG_VERSION")),
        }
    }
}

/// The resolved style of one element, reduced to what this conversion
/// asks of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub display: String,
    pub visibility: String,
    pub font_weight: String,
    pub font_style: String,
    pub text_decoration: String,
    /// Top margin in points.
    pub margin_top: f64,
    /// Font size in points; the ratio to `margin_top` becomes a count
    /// of blank lines.
    pub font_size: f64,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            display: "inline".to_string(),
            visibility: "visible".to_string(),
            font_weight: "normal".to_string(),
            font_style: "normal".to_string(),
            text_decoration: "none".to_string(),
            margin_top: 0.0,
            font_size: 12.0,
        }
    }
}

/// Supplies the resolved style of an element.
///
/// Stands in for `calibre.ebooks.oeb.stylizer.Stylizer`.
pub trait Stylizer {
    fn style(&self, node: Node) -> Style;
}

/// A stylizer that knows only the HTML defaults: which elements are
/// blocks, and which imply bold or italic.
///
/// Enough for documents that carry their formatting in their markup,
/// which is what FB2 can represent anyway. A real CSS cascade would
/// replace this without changing the conversion.
#[derive(Debug, Default, Clone)]
pub struct TagStylizer;

impl Stylizer for TagStylizer {
    fn style(&self, node: Node) -> Style {
        let tag = node.tag_name().name();
        let mut style = Style::default();
        if matches!(
            tag,
            "html"
                | "body"
                | "div"
                | "p"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "ul"
                | "ol"
                | "li"
                | "blockquote"
                | "table"
                | "tr"
                | "td"
                | "th"
                | "section"
                | "article"
        ) {
            style.display = "block".to_string();
        }
        if matches!(tag, "head" | "script" | "style" | "title" | "meta" | "link") {
            style.display = "none".to_string();
        }
        if matches!(
            tag,
            "b" | "strong" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
        ) {
            style.font_weight = "bold".to_string();
        }
        if matches!(tag, "i" | "em" | "cite" | "dfn" | "var") {
            style.font_style = "italic".to_string();
        }
        if matches!(tag, "del" | "s" | "strike") {
            style.text_decoration = "line-through".to_string();
        }
        style
    }
}

/// Converts an image to JPEG for embedding.
///
/// Stands in for `calibre.utils.img.save_cover_data_to`. FB2 only
/// takes JPEG and PNG, so anything else has to be converted or
/// dropped.
pub trait ImageConverter {
    /// Returns the JPEG bytes, or `None` if the image cannot be
    /// converted — in which case it is left out of the file.
    fn to_jpeg(&self, data: &[u8]) -> Option<Vec<u8>>;
}

/// Drops images FB2 cannot carry natively rather than converting them.
///
/// The default, because this crate has no image codec yet. A caller
/// with one implements [`ImageConverter`] instead.
#[derive(Debug, Default, Clone)]
pub struct NoImageConversion;

impl ImageConverter for NoImageConversion {
    fn to_jpeg(&self, _data: &[u8]) -> Option<Vec<u8>> {
        None
    }
}

/// Anything that went wrong that the caller may want to know about.
///
/// The conversion never fails outright: a missing or unparseable spine
/// item is skipped and reported here, matching the Python, which logs
/// and carries on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warnings(pub Vec<String>);

/// Converts an OEB book to FB2 markup.
///
/// Port of the Python `FB2MLizer`.
#[derive(Debug, Default)]
pub struct Fb2Mlizer {
    /// Whether output is currently inside a `<p>`.
    in_p: bool,
    /// OEB href → flat FB2 image id. FB2 images live in one flat
    /// namespace, so they are renumbered to avoid collisions between
    /// same-named files in different directories.
    image_hrefs: Vec<(String, String)>,
    /// Spine href → the TOC entries pointing into it. A `None` key
    /// means the TOC points at the page itself.
    toc: HashMap<String, HashMap<Option<String>, usize>>,
    section_level: i32,
    warnings: Vec<String>,
}

impl Fb2Mlizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Port of the Python `reset_state`.
    fn reset_state(&mut self) {
        self.in_p = false;
        self.image_hrefs.clear();
        self.toc.clear();
        self.section_level = 0;
        self.warnings.clear();
    }

    /// Anything skipped during the last conversion.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Convert a book to a complete FB2 document.
    ///
    /// Port of the Python `extract_content` plus `fb2mlize_spine`.
    /// `date` is the `<date>` written into the document info, which the
    /// Python takes from the clock; passing it in keeps the output
    /// reproducible.
    pub fn extract_content(
        &mut self,
        oeb: &OEBBook,
        opts: &Fb2Options,
        stylizer: &dyn Stylizer,
        images: &dyn ImageConverter,
        date: &str,
        uuid_fallback: &str,
    ) -> String {
        self.reset_state();
        if opts.sectionize == Sectionize::Toc {
            let root = oeb.toc.root.clone();
            self.create_flat_toc(&root.children, 1);
        }

        // The body is generated first: it is what populates the image
        // map that the binaries and the cover then read.
        let cover = self.get_cover(oeb);
        let body = self.get_text(oeb, opts, stylizer);
        let header = self.fb2_header(oeb, opts, &cover, date, uuid_fallback);
        let binaries = self.fb2mlize_images(oeb, images);

        let joined = format!("{header}\n{body}\n{binaries}\n{}", fb2_footer());
        let cleaned = clean_text(&joined, opts.insert_blank_line);
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{cleaned}")
    }

    /// Port of the Python `create_flat_toc`.
    fn create_flat_toc(&mut self, nodes: &[TOCNode], level: usize) {
        for item in nodes {
            let Some(full) = item.href.as_deref() else {
                continue;
            };
            let (href, id) = match full.split_once('#') {
                Some((h, id)) if !id.is_empty() => (h.to_string(), Some(id.to_string())),
                _ => (full.trim_end_matches('#').to_string(), None),
            };
            match id {
                None => {
                    self.toc.insert(href, HashMap::from([(None, 0)]));
                }
                Some(id) => {
                    self.toc.entry(href).or_default().insert(Some(id), level);
                    self.create_flat_toc(&item.children, level + 1);
                }
            }
        }
    }

    /// Assign, or look up, the flat FB2 id for an image href.
    fn image_id(&mut self, href: &str) -> String {
        if let Some((_, id)) = self.image_hrefs.iter().find(|(h, _)| h == href) {
            return id.clone();
        }
        let id = format!("img_{}", self.image_hrefs.len());
        self.image_hrefs.push((href.to_string(), id.clone()));
        id
    }

    /// The `<coverpage>` element, if the book has a cover image.
    ///
    /// Port of the Python `get_cover`: the raster image named by the
    /// metadata, or failing that the first `<img>` on whichever of the
    /// title page or cover page the guide points at.
    fn get_cover(&mut self, oeb: &OEBBook) -> String {
        let from_metadata = oeb
            .metadata
            .get("cover")
            .first()
            .map(|item| item.value.clone())
            .and_then(|id| oeb.manifest.items.get(&id))
            .filter(|item| RASTER_IMAGE_TYPES.contains(&item.media_type.as_str()))
            .map(|item| item.href.clone());

        let cover_href = match from_metadata {
            Some(href) => Some(href),
            None => self.cover_from_guide(oeb),
        };

        let Some(href) = cover_href else {
            return String::new();
        };
        // Only write the tag if the image is actually in the manifest.
        if !oeb.manifest.hrefs.contains_key(&href) {
            return String::new();
        }
        let id = self.image_id(&href);
        format!("<coverpage><image l:href=\"#{id}\"/></coverpage>")
    }

    /// The first image on the guide's title page or cover page.
    fn cover_from_guide(&self, oeb: &OEBBook) -> Option<String> {
        let page = ["titlepage", "cover"]
            .iter()
            .find_map(|name| oeb.guide.references.get(*name))?;
        let page_href = page
            .href
            .split('#')
            .next()
            .unwrap_or(&page.href)
            .to_string();
        let raw = oeb.container.read(&page_href).ok()?;
        let content = String::from_utf8_lossy(&raw).into_owned();
        let doc = Document::parse(&content).ok()?;
        let img = doc
            .descendants()
            .find(|n| n.is_element() && n.tag_name().name() == "img")?;
        let src = img.attribute("src")?;
        Some(resolve_href(&page_href, src))
    }

    /// The `<description>` block.
    ///
    /// Port of the Python `fb2_header`.
    fn fb2_header(
        &self,
        oeb: &OEBBook,
        opts: &Fb2Options,
        cover: &str,
        date: &str,
        uuid_fallback: &str,
    ) -> String {
        let first = |term: &str| -> Option<String> {
            oeb.metadata.get(term).first().map(|i| i.value.clone())
        };
        let title = first("title").unwrap_or_default();
        // calibre canonicalizes the language to ISO-639-1 first; that
        // needs the language tables tracked in #140.
        let lang = first("language")
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| "en".to_string());

        let mut author = String::new();
        for creator in oeb.metadata.get("creator") {
            author.push_str(&format_author(&creator.value));
        }
        if author.is_empty() {
            author =
                "<author><first-name></first-name><last-name></last-name></author>".to_string();
        }

        let tags: Vec<String> = oeb
            .metadata
            .get("subject")
            .iter()
            .map(|i| prepare_string_for_xml(&i.value, false))
            .collect();
        let keywords = if tags.is_empty() {
            String::new()
        } else {
            format!("<keywords>{}</keywords>", tags.join(", "))
        };

        let sequence = match first("series") {
            Some(series) => {
                let index = first("series_index").unwrap_or_else(|| "1".to_string());
                format!(
                    "<sequence name=\"{}\" number=\"{}\"/>",
                    prepare_string_for_xml(&series, true),
                    prepare_string_for_xml(&index, true)
                )
            }
            None => String::new(),
        };

        // The book id is the identifier whose scheme is uuid, or one
        // spelled as a urn.
        let identifiers = oeb.metadata.get("identifier");
        let id = identifiers
            .iter()
            .find(|i| {
                i.attrib
                    .iter()
                    .any(|(k, v)| k.ends_with("scheme") && v.eq_ignore_ascii_case("uuid"))
                    || i.value.starts_with("urn:uuid:")
            })
            .map(|i| i.value.rsplit(':').next().unwrap_or(&i.value).to_string())
            .unwrap_or_else(|| uuid_fallback.to_string());

        let year = first("date")
            .map(|d| {
                format!(
                    "<year>{}</year>",
                    prepare_string_for_xml(d.split('-').next().unwrap_or(""), false)
                )
            })
            .unwrap_or_default();
        let publisher = first("publisher")
            .map(|p| {
                format!(
                    "<publisher>{}</publisher>",
                    prepare_string_for_xml(&p, false)
                )
            })
            .unwrap_or_default();
        let isbn = identifiers
            .iter()
            .find(|i| {
                i.attrib
                    .iter()
                    .any(|(k, v)| k.ends_with("scheme") && v.eq_ignore_ascii_case("isbn"))
            })
            .map(|i| format!("<isbn>{}</isbn>", prepare_string_for_xml(&i.value, false)))
            .unwrap_or_default();

        let comments = first("description")
            .map(|c| {
                format!(
                    "<annotation><p>{}</p></annotation>",
                    prepare_string_for_xml(calibre_utils::html2text::html2text(&c).trim(), false)
                )
            })
            .unwrap_or_default();

        let lines = [
            "<FictionBook xmlns=\"http://www.gribuser.ru/xml/fictionbook/2.0\" xmlns:l=\"http://www.w3.org/1999/xlink\">".to_string(),
            "<description>".to_string(),
            "    <title-info>".to_string(),
            format!("        <genre>{}</genre>", prepare_string_for_xml(&opts.genre, false)),
            format!("        {author}"),
            format!("        <book-title>{}</book-title>", prepare_string_for_xml(&title, false)),
            format!("        {cover}"),
            format!("        <lang>{}</lang>", prepare_string_for_xml(&lang, false)),
            format!("        {keywords}"),
            format!("        {sequence}"),
            format!("        {comments}"),
            "    </title-info>".to_string(),
            "    <document-info>".to_string(),
            format!("        {author}"),
            format!("        <program-used>{}</program-used>", prepare_string_for_xml(&opts.program, false)),
            format!("        <date>{}</date>", prepare_string_for_xml(date, false)),
            format!("        <id>{}</id>", prepare_string_for_xml(&id, false)),
            "        <version>1.0</version>".to_string(),
            "    </document-info>".to_string(),
            "    <publish-info>".to_string(),
            format!("        {publisher}"),
            format!("        {year}"),
            format!("        {isbn}"),
            "    </publish-info>".to_string(),
            "</description>".to_string(),
        ];
        // Blank lines are dropped, as in the Python, so absent
        // metadata leaves no gap.
        lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The `<body>`, walking every spine item.
    ///
    /// Port of the Python `get_text`.
    fn get_text(&mut self, oeb: &OEBBook, opts: &Fb2Options, stylizer: &dyn Stylizer) -> String {
        let mut text = vec!["<body>".to_string()];
        if opts.sectionize == Sectionize::Nothing {
            text.push("<section>".to_string());
            self.section_level += 1;
        }

        for spine_item in &oeb.spine.items {
            let Some(item) = oeb.manifest.items.get(&spine_item.idref) else {
                self.warnings
                    .push(format!("spine item not in manifest: {}", spine_item.idref));
                continue;
            };
            let href = item.href.clone();
            let raw = match oeb.container.read(&href) {
                Ok(raw) => raw,
                Err(e) => {
                    self.warnings.push(format!("could not read {href}: {e}"));
                    continue;
                }
            };
            let content = String::from_utf8_lossy(&raw).into_owned();
            let doc = match Document::parse(&content) {
                Ok(doc) => doc,
                Err(e) => {
                    self.warnings.push(format!("could not parse {href}: {e}"));
                    continue;
                }
            };

            let page_section_open = opts.sectionize == Sectionize::Files
                || self
                    .toc
                    .get(&href)
                    .is_some_and(|entries| entries.contains_key(&None));
            if page_section_open {
                text.push("<section>".to_string());
                self.section_level += 1;
            }

            let body = doc
                .descendants()
                .find(|n| n.is_element() && n.tag_name().name() == "body")
                .unwrap_or(doc.root_element());
            let mut out = Vec::new();
            self.dump_text(body, stylizer, oeb, &href, opts, &[], &mut out);
            text.extend(out);

            if page_section_open {
                text.push("</section>".to_string());
                self.section_level -= 1;
            }
        }

        while self.section_level > 0 {
            text.push("</section>".to_string());
            self.section_level -= 1;
        }
        text.push("</body>".to_string());
        text.join("")
    }

    /// The `<binary>` elements for every image the body referenced.
    ///
    /// Port of the Python `fb2mlize_images`.
    fn fb2mlize_images(&mut self, oeb: &OEBBook, images: &dyn ImageConverter) -> String {
        let mut out = Vec::new();
        // Ordered by the flat id, so output does not depend on how the
        // manifest happens to be iterated.
        let referenced = self.image_hrefs.clone();
        for (href, id) in referenced {
            let Some(item) = oeb
                .manifest
                .hrefs
                .get(&href)
                .and_then(|item_id| oeb.manifest.items.get(item_id))
            else {
                continue;
            };
            if !RASTER_IMAGE_TYPES.contains(&item.media_type.as_str()) {
                continue;
            }
            let data = match oeb.container.read(&href) {
                Ok(data) => data,
                Err(e) => {
                    self.warnings
                        .push(format!("could not include {href} because {e}"));
                    continue;
                }
            };
            let (content_type, data) = if NATIVE_IMAGE_TYPES.contains(&item.media_type.as_str()) {
                (item.media_type.clone(), data)
            } else {
                match images.to_jpeg(&data) {
                    Some(jpeg) => ("image/jpeg".to_string(), jpeg),
                    None => {
                        self.warnings.push(format!(
                            "could not include {href} because {} needs conversion to JPEG",
                            item.media_type
                        ));
                        continue;
                    }
                }
            };
            let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
            // Wrapped, because a megabyte on one line makes the file
            // unreadable and some readers baulk.
            let wrapped: Vec<String> = encoded
                .as_bytes()
                .chunks(72)
                .map(|c| String::from_utf8_lossy(c).into_owned())
                .collect();
            out.push(format!(
                "<binary id=\"{id}\" content-type=\"{content_type}\">{}</binary>",
                wrapped.join("\n")
            ));
        }
        out.join("\n")
    }

    // -- the tag-stack helpers ---------------------------------------

    /// Port of the Python `ensure_p`.
    fn ensure_p(&mut self) -> (Vec<String>, Vec<String>) {
        if self.in_p {
            (vec![], vec![])
        } else {
            self.in_p = true;
            (vec!["<p>".to_string()], vec!["p".to_string()])
        }
    }

    /// Close everything up to and including the open `<p>`, then reopen
    /// it and what was closed with it.
    ///
    /// Port of the Python `close_open_p`.
    fn close_open_p(&mut self, tags: &[String]) -> (Vec<String>, bool) {
        let mut text = vec![String::new()];
        if self.in_p {
            let mut closed = Vec::new();
            for t in tags.iter().rev() {
                text.push(format!("</{t}>"));
                closed.push(t.clone());
                if t == "p" {
                    break;
                }
            }
            closed.reverse();
            for t in closed {
                text.push(format!("<{t}>"));
            }
            (text, false)
        } else {
            text.push("<p>".to_string());
            self.in_p = true;
            (text, true)
        }
    }

    /// Port of the Python `handle_simple_tag`.
    fn handle_simple_tag(&mut self, tag: &str, tags: &[String]) -> (Vec<String>, Vec<String>) {
        if tags.iter().any(|t| t == tag) {
            return (vec![], vec![]);
        }
        let (mut out, mut new_tags) = self.ensure_p();
        out.push(format!("<{tag}>"));
        new_tags.push(tag.to_string());
        (out, new_tags)
    }

    /// Port of the Python `close_tags`.
    fn close_tags(&mut self, tags: &[String]) -> Vec<String> {
        let mut text = Vec::new();
        for tag in tags {
            text.push(format!("</{tag}>"));
            if tag == "p" {
                self.in_p = false;
            }
        }
        text
    }

    /// Walk one element, emitting FB2 markup, then recurse.
    ///
    /// Port of the Python `dump_text`.
    #[allow(clippy::too_many_arguments)]
    fn dump_text(
        &mut self,
        elem: Node,
        stylizer: &dyn Stylizer,
        oeb: &OEBBook,
        page_href: &str,
        opts: &Fb2Options,
        tag_stack: &[String],
        out: &mut Vec<String>,
    ) {
        if !elem.is_element() {
            return;
        }
        // Content outside the XHTML namespace is not converted, but
        // text following such an element still is.
        let ns = elem.tag_name().namespace();
        if !(ns.is_none() || ns == Some(XHTML_NS)) {
            if let Some(tail) = tail_text(elem) {
                out.push(tail);
            }
            return;
        }

        let style = stylizer.style(elem);
        if matches!(
            style.display.as_str(),
            "none" | "oeb-page-head" | "oeb-page-foot"
        ) || style.visibility == "hidden"
        {
            if let Some(tail) = tail_text(elem) {
                out.push(tail);
            }
            return;
        }

        let mut tags: Vec<String> = Vec::new();
        let tag = elem.tag_name().name().to_string();
        // Blank lines above this element, from its margin.
        let ems = if style.font_size > 0.0 {
            ((style.margin_top / style.font_size) - 1.0)
                .round()
                .max(0.0) as usize
        } else {
            0
        };

        if opts.sectionize == Sectionize::Toc {
            // A section may only be a child of another section, so this
            // only fires at the top of a page.
            if tag_stack.is_empty() {
                let mut newlevel = 0usize;
                if let Some(entry) = self.toc.get(page_href).cloned() {
                    if entry.contains_key(&None)
                        && tag != "body"
                        && own_text(elem).is_some_and(|t| !t.is_empty())
                    {
                        newlevel = 1;
                        // Only the first element of the page becomes a
                        // title, so the marker is consumed.
                        self.toc.remove(page_href);
                    }
                    if newlevel == 0 {
                        if let Some(id) = elem.attribute("id") {
                            newlevel = entry.get(&Some(id.to_string())).copied().unwrap_or(0);
                        }
                    }
                }
                if newlevel > 0 {
                    while newlevel as i32 <= self.section_level {
                        out.push("</section>".to_string());
                        self.section_level -= 1;
                    }
                    out.push("<section>".to_string());
                    self.section_level += 1;
                    out.push("<title>".to_string());
                    tags.push("title".to_string());
                }
            }
            if self.section_level == 0 {
                // FB2 requires a section around body content.
                out.push("<section>".to_string());
                self.section_level += 1;
            }
        }

        // One XHTML tag can produce several FB2 ones, so these are
        // independent ifs rather than a chain.
        if tag == "img" {
            if let Some(src) = elem.attribute("src").filter(|s| !s.is_empty()) {
                let href = resolve_href(page_href, src);
                if oeb.manifest.hrefs.contains_key(&href) {
                    let id = self.image_id(&href);
                    let (p_txt, p_tag) = self.ensure_p();
                    out.extend(p_txt);
                    tags.extend(p_tag);
                    out.push(format!("<image l:href=\"#{id}\"/>"));
                } else {
                    self.warnings
                        .push(format!("ignoring image not in manifest: {href}"));
                }
            }
        }
        if tag == "br" || tag == "hr" || ems >= 1 {
            let multiplier = ems.max(1);
            if self.in_p {
                let mut closed = Vec::new();
                let open_tags: Vec<String> = tag_stack.iter().chain(tags.iter()).cloned().collect();
                for t in open_tags.iter().rev() {
                    out.push(format!("</{t}>"));
                    closed.push(t.clone());
                    if t == "p" {
                        break;
                    }
                }
                out.push("<empty-line/>".repeat(multiplier));
                closed.reverse();
                for t in closed {
                    out.push(format!("<{t}>"));
                }
            } else {
                out.push("<empty-line/>".repeat(multiplier));
            }
        }
        if matches!(tag.as_str(), "div" | "li" | "p") {
            let stack: Vec<String> = tag_stack.iter().chain(tags.iter()).cloned().collect();
            let (p_text, added_p) = self.close_open_p(&stack);
            out.extend(p_text);
            if added_p {
                tags.push("p".to_string());
            }
        }
        if tag == "a" {
            if let Some(href) = elem.attribute("href").filter(|h| !h.is_empty()) {
                // Only external links survive: FB2's internal links
                // need note targets this conversion does not build.
                if is_external(href) {
                    let (p_txt, p_tag) = self.ensure_p();
                    out.extend(p_txt);
                    tags.extend(p_tag);
                    out.push(format!(
                        "<a l:href=\"{}\">",
                        prepare_string_for_xml(href, true)
                    ));
                    tags.push("a".to_string());
                }
            }
        }
        for (matches_tag, styled, fb2_tag) in [
            (
                tag == "b",
                matches!(style.font_weight.as_str(), "bold" | "bolder"),
                "strong",
            ),
            (tag == "i", style.font_style == "italic", "emphasis"),
            (
                matches!(tag.as_str(), "del" | "strike"),
                style.text_decoration == "line-through",
                "strikethrough",
            ),
            (tag == "sub", false, "sub"),
            (tag == "sup", false, "sup"),
        ] {
            if matches_tag || styled {
                let stack: Vec<String> = tag_stack.iter().chain(tags.iter()).cloned().collect();
                let (s_out, s_tags) = self.handle_simple_tag(fb2_tag, &stack);
                out.extend(s_out);
                tags.extend(s_tags);
            }
        }

        if let Some(text) = own_text(elem) {
            if !text.is_empty() {
                let wrap = !self.in_p;
                if wrap {
                    out.push("<p>".to_string());
                }
                out.push(prepare_string_for_xml(&text, false));
                if wrap {
                    out.push("</p>".to_string());
                }
            }
        }

        let child_stack: Vec<String> = tag_stack.iter().chain(tags.iter()).cloned().collect();
        for child in elem.children() {
            if child.is_element() {
                self.dump_text(child, stylizer, oeb, page_href, opts, &child_stack, out);
            }
        }

        tags.reverse();
        let closing = self.close_tags(&tags);
        out.extend(closing);

        if let Some(tail) = tail_text(elem) {
            let wrap = !self.in_p;
            if wrap {
                out.push("<p>".to_string());
            }
            out.push(prepare_string_for_xml(&tail, false));
            if wrap {
                out.push("</p>".to_string());
            }
        }
    }
}

/// The text directly inside an element, before any child element.
///
/// lxml's `.text`; roxmltree models it as the first child when that
/// child is text.
fn own_text(elem: Node) -> Option<String> {
    let first = elem.first_child()?;
    if first.is_text() {
        first.text().map(str::to_string)
    } else {
        None
    }
}

/// The text following an element, before the next one.
///
/// lxml's `.tail`.
fn tail_text(elem: Node) -> Option<String> {
    let next = elem.next_sibling()?;
    if next.is_text() {
        next.text().map(str::to_string).filter(|t| !t.is_empty())
    } else {
        None
    }
}

/// Whether a link points outside the book.
///
/// Port of the Python's `urlparse(href).netloc` test.
fn is_external(href: &str) -> bool {
    match href.split_once("://") {
        Some((scheme, rest)) => !scheme.is_empty() && !rest.is_empty(),
        None => href.starts_with("//") && href.len() > 2,
    }
}

/// Resolve an `src` against the page that carried it.
///
/// Stands in for the Python's `page.abshref` plus `urlnormalize`.
fn resolve_href(page_href: &str, src: &str) -> String {
    if src.starts_with('/') || is_external(src) {
        return src.to_string();
    }
    let base = match page_href.rfind('/') {
        Some(i) => &page_href[..i + 1],
        None => "",
    };
    let joined = format!("{base}{src}");
    // Collapse `a/../b` and `./b`, which OEB hrefs use freely.
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// The document's closing tag.
///
/// Port of the Python `fb2_footer`.
pub fn fb2_footer() -> &'static str {
    "</FictionBook>"
}

/// Build the `<author>` element for one creator.
///
/// Port of the Python's name splitting: one word is a surname, two are
/// first and last, and anything longer puts the middle words in
/// `<middle-name>`.
fn format_author(name: &str) -> String {
    let parts: Vec<&str> = name.split(' ').collect();
    let (first, middle, last) = match parts.len() {
        0 => ("", String::new(), ""),
        1 => ("", String::new(), parts[0]),
        2 => (parts[0], String::new(), parts[1]),
        _ => (
            parts[0],
            parts[1..parts.len() - 1].join(" "),
            parts[parts.len() - 1],
        ),
    };
    let mut out = String::from("<author>");
    out.push_str(&format!(
        "<first-name>{}</first-name>",
        prepare_string_for_xml(first, false)
    ));
    if !middle.is_empty() {
        out.push_str(&format!(
            "<middle-name>{}</middle-name>",
            prepare_string_for_xml(&middle, false)
        ));
    }
    out.push_str(&format!(
        "<last-name>{}</last-name>",
        prepare_string_for_xml(last, false)
    ));
    out.push_str("</author>");
    out
}

/// Tidy generated markup: drop empty elements, collapse runs of empty
/// paragraphs into line breaks, and put block elements on their own
/// lines.
///
/// Port of the Python `clean_text`, pattern for pattern and in the same
/// order — the passes are not independent, so the order is part of the
/// behaviour.
pub fn clean_text(text: &str, insert_blank_line: bool) -> String {
    use std::sync::OnceLock;

    struct Patterns {
        empty_inline: Vec<Regex>,
        space_before_p_close: Regex,
        empty_paragraph_runs: Regex,
        empty_paragraph: Regex,
        paragraph_after_paragraph: Regex,
        paragraph_close: Regex,
        space_before_title_close: Regex,
        empty_title: Regex,
        paragraph_after_title: Regex,
        empty_line_after_block: Regex,
        paragraph_after_empty_line: Regex,
        empty_section: Regex,
        before_section_open: Regex,
        after_section_open: Regex,
        before_section_close: Regex,
        after_section_close: Regex,
    }

    static PATTERNS: OnceLock<Patterns> = OnceLock::new();
    let p = PATTERNS.get_or_init(|| Patterns {
        // Rust's regex crate has no backreferences, so calibre's single
        // `<(strong|...)>(\s*)</\1>` becomes one pattern per tag.
        empty_inline: ["strong", "emphasis", "strikethrough", "sub", "sup"]
            .iter()
            .map(|t| Regex::new(&format!(r"(?m)<{t}>(\s*)</{t}>")).expect("valid regex"))
            .collect(),
        // calibre spells this one `(?ma)`, i.e. ASCII-only whitespace,
        // unlike every other pattern here — so a non-breaking space
        // before `</p>` is deliberately left alone.
        space_before_p_close: Regex::new(r"(?m)[ \t\n\r\x0b\x0c]+</p>").expect("valid regex"),
        empty_paragraph_runs: Regex::new(r"(?m)(?:<p></p>\s*){3,}").expect("valid regex"),
        empty_paragraph: Regex::new(r"(?m)<p></p>\s*").expect("valid regex"),
        paragraph_after_paragraph: Regex::new(r"(?m)</p>\s*<p>").expect("valid regex"),
        paragraph_close: Regex::new(r"(?m)</p>").expect("valid regex"),
        space_before_title_close: Regex::new(r"(?m)\s+</title>").expect("valid regex"),
        empty_title: Regex::new(r"(?m)<title></title>\s*").expect("valid regex"),
        paragraph_after_title: Regex::new(r"(?m)</title>\s*<p>").expect("valid regex"),
        empty_line_after_block: Regex::new(r"(?m)</(p|title)>\s*<empty-line/>")
            .expect("valid regex"),
        paragraph_after_empty_line: Regex::new(r"(?m)<empty-line/>\s*<p>").expect("valid regex"),
        empty_section: Regex::new(r"(?m)<section>\s*</section>").expect("valid regex"),
        before_section_open: Regex::new(r"(?m)\s*<section>").expect("valid regex"),
        after_section_open: Regex::new(r"(?m)<section>\s*").expect("valid regex"),
        before_section_close: Regex::new(r"(?m)\s*</section>").expect("valid regex"),
        after_section_close: Regex::new(r"(?m)</section>\s*").expect("valid regex"),
    });

    let mut text = text.to_string();
    for re in &p.empty_inline {
        text = re.replace_all(&text, "$1").into_owned();
    }
    text = p
        .space_before_p_close
        .replace_all(&text, "</p>")
        .into_owned();
    text = p
        .empty_paragraph_runs
        .replace_all(&text, "<empty-line/>")
        .into_owned();
    text = p.empty_paragraph.replace_all(&text, "").into_owned();
    text = p
        .paragraph_after_paragraph
        .replace_all(&text, "</p>\n<p>")
        .into_owned();
    if insert_blank_line {
        text = p
            .paragraph_close
            .replace_all(&text, "</p><empty-line/>")
            .into_owned();
    }
    text = p
        .space_before_title_close
        .replace_all(&text, "</title>")
        .into_owned();
    text = p.empty_title.replace_all(&text, "").into_owned();
    text = p
        .paragraph_after_title
        .replace_all(&text, "</title>\n<p>")
        .into_owned();
    text = p
        .empty_line_after_block
        .replace_all(&text, "</$1>\n<empty-line/>")
        .into_owned();
    text = p
        .paragraph_after_empty_line
        .replace_all(&text, "<empty-line/>\n<p>")
        .into_owned();
    text = p.empty_section.replace_all(&text, "").into_owned();
    text = p
        .before_section_open
        .replace_all(&text, "\n<section>")
        .into_owned();
    text = p
        .after_section_open
        .replace_all(&text, "<section>\n")
        .into_owned();
    text = p
        .before_section_close
        .replace_all(&text, "\n</section>")
        .into_owned();
    text = p
        .after_section_close
        .replace_all(&text, "</section>\n")
        .into_owned();
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::container::Container;
    use crate::oeb::manifest::ManifestItem;
    use crate::oeb::spine::SpineItem;
    use anyhow::Result;
    use std::collections::HashMap as Map;

    /// A container serving content from memory.
    #[derive(Default)]
    struct MemContainer(Map<String, Vec<u8>>);

    impl Container for MemContainer {
        fn read(&self, path: &str) -> Result<Vec<u8>> {
            self.0
                .get(path)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no such part: {path}"))
        }
        fn write(&mut self, path: &str, data: &[u8]) -> Result<()> {
            self.0.insert(path.to_string(), data.to_vec());
            Ok(())
        }
        fn exists(&self, path: &str) -> bool {
            self.0.contains_key(path)
        }
        fn namelist(&self) -> Result<Vec<String>> {
            Ok(self.0.keys().cloned().collect())
        }
    }

    fn book(parts: &[(&str, &str)]) -> OEBBook {
        let mut container = MemContainer::default();
        for (name, content) in parts {
            container
                .0
                .insert((*name).to_string(), content.as_bytes().to_vec());
        }
        let mut oeb = OEBBook::new(Box::new(container));
        oeb.metadata.add("title", "A Book");
        oeb.metadata.add("creator", "Jane Q Public");
        oeb.metadata.add("language", "en");
        for (i, (name, _)) in parts.iter().enumerate() {
            let id = format!("item{i}");
            oeb.manifest.items.insert(
                id.clone(),
                ManifestItem::new(&id, name, "application/xhtml+xml"),
            );
            oeb.manifest.hrefs.insert((*name).to_string(), id.clone());
            oeb.spine.items.push(SpineItem::new(&id, true));
        }
        oeb
    }

    fn convert(oeb: &OEBBook, opts: &Fb2Options) -> String {
        Fb2Mlizer::new().extract_content(
            oeb,
            opts,
            &TagStylizer,
            &NoImageConversion,
            "1.1.2010",
            "fallback-uuid",
        )
    }

    const PAGE: &str = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body>
<p>Hello <b>bold</b> and <i>italic</i>.</p>
<p>Second paragraph.</p>
</body></html>"#;

    #[test]
    fn produces_a_well_formed_document() {
        let oeb = book(&[("index.html", PAGE)]);
        let out = convert(&oeb, &Fb2Options::default());
        assert!(out.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"));
        let doc = Document::parse(&out).expect("the FB2 output parses");
        assert_eq!(doc.root_element().tag_name().name(), "FictionBook");
    }

    #[test]
    fn carries_the_metadata_into_the_description() {
        let mut oeb = book(&[("index.html", PAGE)]);
        oeb.metadata.add("publisher", "A Press");
        oeb.metadata.add("date", "1927-03-04");
        oeb.metadata.add("subject", "fiction");
        oeb.metadata.add("subject", "classics");
        let out = convert(&oeb, &Fb2Options::default());
        let doc = Document::parse(&out).unwrap();
        let text_of = |name: &str| -> Option<String> {
            doc.descendants()
                .find(|n| n.tag_name().name() == name)
                .and_then(|n| n.text())
                .map(str::to_string)
        };
        assert_eq!(text_of("book-title").as_deref(), Some("A Book"));
        assert_eq!(text_of("lang").as_deref(), Some("en"));
        assert_eq!(text_of("publisher").as_deref(), Some("A Press"));
        assert_eq!(text_of("year").as_deref(), Some("1927"));
        assert_eq!(text_of("keywords").as_deref(), Some("fiction, classics"));
        assert_eq!(text_of("date").as_deref(), Some("1.1.2010"));
        assert_eq!(text_of("id").as_deref(), Some("fallback-uuid"));
    }

    #[test]
    fn author_names_split_into_their_parts() {
        assert_eq!(
            format_author("Plato"),
            "<author><first-name></first-name><last-name>Plato</last-name></author>"
        );
        assert_eq!(
            format_author("Jane Austen"),
            "<author><first-name>Jane</first-name><last-name>Austen</last-name></author>"
        );
        assert_eq!(
            format_author("Jane Q Public"),
            "<author><first-name>Jane</first-name><middle-name>Q</middle-name><last-name>Public</last-name></author>"
        );
        assert_eq!(
            format_author("A B C D"),
            "<author><first-name>A</first-name><middle-name>B C</middle-name><last-name>D</last-name></author>"
        );
    }

    #[test]
    fn a_book_with_no_creator_still_gets_an_author_element() {
        let mut oeb = book(&[("index.html", PAGE)]);
        oeb.metadata = crate::oeb::metadata::Metadata::new();
        oeb.metadata.add("title", "Anonymous");
        let out = convert(&oeb, &Fb2Options::default());
        assert!(out.contains("<author><first-name></first-name><last-name></last-name></author>"));
        Document::parse(&out).expect("still well formed");
    }

    #[test]
    fn a_uuid_identifier_becomes_the_document_id() {
        let mut oeb = book(&[("index.html", PAGE)]);
        let mut attrib = Map::new();
        attrib.insert("scheme".to_string(), "uuid".to_string());
        oeb.metadata
            .add_with_attrib("identifier", "urn:uuid:1234-5678", attrib);
        let mut isbn = Map::new();
        isbn.insert("scheme".to_string(), "ISBN".to_string());
        oeb.metadata
            .add_with_attrib("identifier", "9780000000000", isbn);
        let out = convert(&oeb, &Fb2Options::default());
        assert!(out.contains("<id>1234-5678</id>"), "{out}");
        assert!(out.contains("<isbn>9780000000000</isbn>"), "{out}");
    }

    #[test]
    fn inline_formatting_becomes_fb2_tags() {
        let oeb = book(&[("index.html", PAGE)]);
        let out = convert(&oeb, &Fb2Options::default());
        assert!(out.contains("<strong>bold</strong>"), "{out}");
        assert!(out.contains("<emphasis>italic</emphasis>"), "{out}");
    }

    #[test]
    fn text_is_escaped() {
        let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>a &amp; b &lt; c</p></body></html>"#;
        let oeb = book(&[("index.html", page)]);
        let out = convert(&oeb, &Fb2Options::default());
        let doc = Document::parse(&out).expect("escaped output parses");
        let text: String = doc
            .descendants()
            .filter(|n| n.is_text())
            .filter_map(|n| n.text())
            .collect();
        assert!(text.contains("a & b < c"), "{text}");
    }

    #[test]
    fn the_cover_falls_back_to_the_first_image_on_the_title_page() {
        let cover_page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><div><img src="images/cover.jpg"/></div></body></html>"#;
        let mut oeb = book(&[("text/index.html", PAGE)]);
        oeb.container
            .write("text/cover.html", cover_page.as_bytes())
            .unwrap();
        oeb.manifest.items.insert(
            "coverimg".to_string(),
            ManifestItem::new("coverimg", "text/images/cover.jpg", "image/jpeg"),
        );
        oeb.manifest
            .hrefs
            .insert("text/images/cover.jpg".to_string(), "coverimg".to_string());
        oeb.container
            .write("text/images/cover.jpg", b"jpegdata")
            .unwrap();
        oeb.guide.references.insert(
            "titlepage".to_string(),
            crate::oeb::guide::Reference::new("titlepage", None, "text/cover.html"),
        );

        let out = convert(&oeb, &Fb2Options::default());
        assert!(
            out.contains("<coverpage><image l:href=\"#img_0\"/></coverpage>"),
            "{out}"
        );
        assert!(
            out.contains("<binary id=\"img_0\" content-type=\"image/jpeg\">"),
            "{out}"
        );
    }

    #[test]
    fn the_metadata_cover_wins_over_the_guide() {
        let mut oeb = book(&[("index.html", PAGE)]);
        oeb.manifest.items.insert(
            "c".to_string(),
            ManifestItem::new("c", "real-cover.png", "image/png"),
        );
        oeb.manifest
            .hrefs
            .insert("real-cover.png".to_string(), "c".to_string());
        oeb.container.write("real-cover.png", b"png").unwrap();
        oeb.metadata.add("cover", "c");
        oeb.guide.references.insert(
            "cover".to_string(),
            crate::oeb::guide::Reference::new("cover", None, "nonexistent.html"),
        );

        let out = convert(&oeb, &Fb2Options::default());
        assert!(out.contains("<coverpage>"), "{out}");
        assert!(out.contains("content-type=\"image/png\""), "{out}");
    }

    #[test]
    fn a_book_with_no_cover_writes_no_coverpage() {
        let oeb = book(&[("index.html", PAGE)]);
        let out = convert(&oeb, &Fb2Options::default());
        assert!(!out.contains("<coverpage>"), "{out}");
    }

    #[test]
    fn images_are_renumbered_and_embedded() {
        let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p><img src="images/a.png"/></p></body></html>"#;
        let mut oeb = book(&[("text/index.html", page)]);
        oeb.manifest.items.insert(
            "img1".to_string(),
            ManifestItem::new("img1", "text/images/a.png", "image/png"),
        );
        oeb.manifest
            .hrefs
            .insert("text/images/a.png".to_string(), "img1".to_string());
        oeb.container
            .write("text/images/a.png", b"\x89PNG-not-really")
            .unwrap();

        let out = convert(&oeb, &Fb2Options::default());
        assert!(out.contains("<image l:href=\"#img_0\"/>"), "{out}");
        assert!(
            out.contains("<binary id=\"img_0\" content-type=\"image/png\">"),
            "{out}"
        );
        Document::parse(&out).expect("parses");
    }

    #[test]
    fn an_image_outside_the_manifest_is_reported_not_embedded() {
        let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p><img src="missing.png"/></p></body></html>"#;
        let oeb = book(&[("index.html", page)]);
        let mut mlizer = Fb2Mlizer::new();
        let out = mlizer.extract_content(
            &oeb,
            &Fb2Options::default(),
            &TagStylizer,
            &NoImageConversion,
            "1.1.2010",
            "u",
        );
        assert!(!out.contains("<image"), "{out}");
        assert!(
            mlizer.warnings().iter().any(|w| w.contains("missing.png")),
            "{:?}",
            mlizer.warnings()
        );
    }

    #[test]
    fn an_unconvertible_image_is_left_out_with_a_warning() {
        let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p><img src="a.gif"/></p></body></html>"#;
        let mut oeb = book(&[("index.html", page)]);
        oeb.manifest.items.insert(
            "img1".to_string(),
            ManifestItem::new("img1", "a.gif", "image/gif"),
        );
        oeb.manifest
            .hrefs
            .insert("a.gif".to_string(), "img1".to_string());
        oeb.container.write("a.gif", b"GIF89a").unwrap();

        let mut mlizer = Fb2Mlizer::new();
        let out = mlizer.extract_content(
            &oeb,
            &Fb2Options::default(),
            &TagStylizer,
            &NoImageConversion,
            "1.1.2010",
            "u",
        );
        // The reference survives; only the payload is missing, which is
        // what calibre does when the conversion raises.
        assert!(out.contains("<image l:href=\"#img_0\"/>"), "{out}");
        assert!(!out.contains("<binary"), "{out}");
        assert!(mlizer.warnings().iter().any(|w| w.contains("JPEG")));
    }

    #[test]
    fn only_external_links_are_kept() {
        let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body>
<p><a href="https://example.com/x">out</a> and <a href="chapter2.html">in</a></p>
</body></html>"#;
        let oeb = book(&[("index.html", page)]);
        let out = convert(&oeb, &Fb2Options::default());
        assert!(
            out.contains("<a l:href=\"https://example.com/x\">"),
            "{out}"
        );
        assert!(!out.contains("chapter2.html"), "{out}");
        // The inner text of the internal link survives, just not the link.
        assert!(out.contains("in"), "{out}");
    }

    #[test]
    fn is_external_recognises_the_forms_urlparse_does() {
        assert!(is_external("https://example.com/x"));
        assert!(is_external("http://example.com"));
        assert!(is_external("//example.com/x"));
        assert!(!is_external("chapter2.html"));
        assert!(!is_external("#anchor"));
        assert!(!is_external("../images/a.png"));
        assert!(!is_external("mailto:a@b.c"), "no netloc, so not external");
    }

    #[test]
    fn hrefs_resolve_against_the_page() {
        assert_eq!(
            resolve_href("text/index.html", "images/a.png"),
            "text/images/a.png"
        );
        assert_eq!(
            resolve_href("text/index.html", "../images/a.png"),
            "images/a.png"
        );
        assert_eq!(resolve_href("index.html", "a.png"), "a.png");
        assert_eq!(resolve_href("text/index.html", "./a.png"), "text/a.png");
        assert_eq!(resolve_href("text/index.html", "/abs/a.png"), "/abs/a.png");
    }

    #[test]
    fn sectionize_nothing_wraps_the_book_in_one_section() {
        let oeb = book(&[("a.html", PAGE), ("b.html", PAGE)]);
        let out = convert(&oeb, &Fb2Options::default());
        assert_eq!(out.matches("<section>").count(), 1, "{out}");
        Document::parse(&out).expect("parses");
    }

    #[test]
    fn sectionize_files_opens_one_section_per_spine_item() {
        let oeb = book(&[("a.html", PAGE), ("b.html", PAGE)]);
        let opts = Fb2Options {
            sectionize: Sectionize::Files,
            ..Default::default()
        };
        let out = convert(&oeb, &opts);
        assert_eq!(out.matches("<section>").count(), 2, "{out}");
        Document::parse(&out).expect("parses");
    }

    #[test]
    fn sectionize_toc_makes_titles_from_toc_entries() {
        let mut oeb = book(&[("a.html", PAGE), ("b.html", PAGE)]);
        oeb.toc
            .root
            .add(TOCNode::new(Some("One".into()), Some("a.html".into())));
        oeb.toc
            .root
            .add(TOCNode::new(Some("Two".into()), Some("b.html".into())));
        let opts = Fb2Options {
            sectionize: Sectionize::Toc,
            ..Default::default()
        };
        let out = convert(&oeb, &opts);
        assert!(out.contains("<title>"), "{out}");
        Document::parse(&out).expect("parses");
    }

    #[test]
    fn a_missing_or_broken_spine_item_is_skipped_and_reported() {
        let mut oeb = book(&[("good.html", PAGE)]);
        // A manifest entry whose file is not in the container.
        oeb.manifest.items.insert(
            "gone".to_string(),
            ManifestItem::new("gone", "gone.html", "application/xhtml+xml"),
        );
        oeb.spine.items.push(SpineItem::new("gone", true));
        // And one whose content does not parse.
        oeb.manifest.items.insert(
            "bad".to_string(),
            ManifestItem::new("bad", "bad.html", "application/xhtml+xml"),
        );
        oeb.container.write("bad.html", b"<not <xml").unwrap();
        oeb.spine.items.push(SpineItem::new("bad", true));

        let mut mlizer = Fb2Mlizer::new();
        let out = mlizer.extract_content(
            &oeb,
            &Fb2Options::default(),
            &TagStylizer,
            &NoImageConversion,
            "1.1.2010",
            "u",
        );
        Document::parse(&out).expect("the rest of the book still converts");
        assert_eq!(mlizer.warnings().len(), 2, "{:?}", mlizer.warnings());
    }

    #[test]
    fn an_empty_book_produces_a_valid_document() {
        let oeb = book(&[]);
        let out = convert(&oeb, &Fb2Options::default());
        Document::parse(&out).expect("parses");
    }

    #[test]
    fn insert_blank_line_puts_a_break_after_every_paragraph() {
        let oeb = book(&[("index.html", PAGE)]);
        let opts = Fb2Options {
            insert_blank_line: true,
            ..Default::default()
        };
        let out = convert(&oeb, &opts);
        assert!(out.contains("<empty-line/>"), "{out}");
        Document::parse(&out).expect("parses");
    }

    #[test]
    fn hidden_content_is_dropped_but_its_tail_is_kept() {
        struct HideDivs;
        impl Stylizer for HideDivs {
            fn style(&self, node: Node) -> Style {
                let mut s = TagStylizer.style(node);
                if node.tag_name().name() == "div" {
                    s.display = "none".to_string();
                }
                s
            }
        }
        let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>keep<div>drop</div>tail</p></body></html>"#;
        let oeb = book(&[("index.html", page)]);
        let out = Fb2Mlizer::new().extract_content(
            &oeb,
            &Fb2Options::default(),
            &HideDivs,
            &NoImageConversion,
            "1.1.2010",
            "u",
        );
        assert!(!out.contains("drop"), "{out}");
        assert!(out.contains("keep") && out.contains("tail"), "{out}");
    }

    // -- clean_text --------------------------------------------------

    #[test]
    fn clean_text_removes_empty_inline_tags_but_keeps_their_spacing() {
        assert_eq!(clean_text("<strong> </strong>", false), " ");
        assert_eq!(clean_text("<emphasis></emphasis>", false), "");
        assert_eq!(clean_text("<sub>\n</sub>", false), "\n");
        // A non-empty one is left alone.
        assert_eq!(
            clean_text("<strong>x</strong>", false),
            "<strong>x</strong>"
        );
    }

    #[test]
    fn clean_text_condenses_runs_of_empty_paragraphs() {
        assert_eq!(clean_text("<p></p><p></p><p></p>", false), "<empty-line/>");
        // Fewer than three are simply removed.
        assert_eq!(clean_text("<p></p><p></p>", false), "");
    }

    #[test]
    fn clean_text_separates_blocks_onto_their_own_lines() {
        assert_eq!(clean_text("<p>a</p><p>b</p>", false), "<p>a</p>\n<p>b</p>");
        assert_eq!(
            clean_text("<title>t</title><p>a</p>", false),
            "<title>t</title>\n<p>a</p>"
        );
    }

    #[test]
    fn clean_text_tidies_sections() {
        assert_eq!(clean_text("<section></section>", false), "");
        assert_eq!(
            clean_text("<section><p>a</p></section>", false),
            "\n<section>\n<p>a</p>\n</section>\n"
        );
    }

    #[test]
    fn clean_text_uses_ascii_whitespace_before_a_closing_p() {
        // calibre spells that one pattern `(?ma)`, so a non-breaking
        // space is not stripped even though an ordinary one is.
        assert_eq!(clean_text("<p>a </p>", false), "<p>a</p>");
        assert_eq!(clean_text("<p>a\u{a0}</p>", false), "<p>a\u{a0}</p>");
    }
}
