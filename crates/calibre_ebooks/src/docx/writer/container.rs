//! Assembling the DOCX package: page geometry, the document skeleton,
//! relationships, and writing the zip.
//!
//! Port of `old_src/src/calibre/ebooks/docx/writer/container.py`.
//!
//! Two deviations from the Python, both about testability rather than
//! behaviour:
//!
//! - The conversion `opts` object becomes an explicit [`PageOptions`],
//!   carrying only the six settings this file reads.
//! - The `dcterms:created`/`dcterms:modified` timestamp is a field on
//!   [`DocxWriter`] rather than a call to `utcnow()` at serialization
//!   time, so a written package can be compared byte for byte.

use std::collections::BTreeMap;
use std::io::{Seek, Write};

use zip::write::FileOptions;
use zip::ZipWriter;

use super::xml::Element;
use crate::docx::error::DocxError;
use crate::docx::names::DocxNamespace;
use crate::metadata::authors::authors_to_string;
use crate::metadata::meta::MetaInformation;

/// Page sizes in points, as `(name, width, height)`.
///
/// Port of `PAPER_SIZES` in
/// `old_src/src/calibre/ebooks/pdf/render/common.py`. The B series is
/// reproduced exactly as calibre defines it, including the entries
/// whose width exceeds their height.
#[rustfmt::skip]
pub static PAPER_SIZES: &[(&str, f64, f64)] = &[
    ("a0", 2381.1023622047246, 3367.5590551181103),
    ("a1", 1683.7795275590552, 2381.1023622047246),
    ("a2", 1190.5511811023623, 1683.7795275590552),
    ("a3", 841.8897637795276, 1190.5511811023623),
    ("a4", 595.2755905511812, 841.8897637795276),
    ("a5", 420.9448818897638, 595.2755905511812),
    ("a6", 297.6377952755906, 420.9448818897638),
    ("b0", 2834.645669291339, 4002.51968503937),
    ("b1", 4002.51968503937, 1417.3228346456694),
    ("b2", 1417.3228346456694, 2001.259842519685),
    ("b3", 2001.259842519685, 708.6614173228347),
    ("b4", 708.6614173228347, 1000.6299212598425),
    ("b5", 500.31496062992125, 708.6614173228347),
    ("b6", 354.33070866141736, 500.31496062992125),
    ("legal", 612.0, 1008.0),
    ("letter", 612.0, 792.0),
];

/// The page geometry settings the writer reads from the conversion
/// options, all in points.
#[derive(Debug, Clone, PartialEq)]
pub struct PageOptions {
    /// A key of [`PAPER_SIZES`]. Calibre's default is `letter`.
    pub page_size: String,
    /// `WIDTHxHEIGHT` in points, overriding `page_size` when set.
    pub custom_page_size: Option<String>,
    /// DOCX-specific margins. A zero here means "fall back to the
    /// conversion-wide margin", which is how calibre spells "unset".
    pub docx_margins: Margins,
    /// The conversion-wide margins used for that fallback.
    pub margins: Margins,
    /// Port of the `preserve_cover_aspect_ratio` conversion option
    /// (`docx_output.py`, `recommended_value=False`). Read by
    /// `from_html::convert`'s cover-image resolution, threaded into
    /// `ImagesManager::create_cover_markup`.
    pub preserve_cover_aspect_ratio: bool,
}

/// Page margins in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Margins {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl Margins {
    pub fn uniform(value: f64) -> Self {
        Self {
            left: value,
            top: value,
            right: value,
            bottom: value,
        }
    }
}

impl Default for PageOptions {
    /// Calibre's recommended values: letter paper, one-inch margins.
    fn default() -> Self {
        Self {
            page_size: "letter".to_string(),
            custom_page_size: None,
            docx_margins: Margins::uniform(72.0),
            margins: Margins::uniform(72.0),
            preserve_cover_aspect_ratio: false,
        }
    }
}

impl PageOptions {
    /// The page size in points.
    ///
    /// Port of the Python `page_size`. An unknown name falls back to
    /// letter rather than raising, since the value comes from a
    /// command line.
    pub fn size(&self) -> (f64, f64) {
        if let Some(custom) = &self.custom_page_size {
            if let Some((w, h)) = custom.split_once('x') {
                if let (Ok(w), Ok(h)) = (w.trim().parse::<f64>(), h.trim().parse::<f64>()) {
                    return (w, h);
                }
            }
        }
        let name = self.page_size.to_lowercase();
        PAPER_SIZES
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, w, h)| (*w, *h))
            .unwrap_or((612.0, 792.0))
    }

    /// One margin, with the docx-specific value taking precedence over
    /// the conversion-wide one.
    ///
    /// Port of the Python `page_margin`.
    pub fn margin(&self, which: Edge) -> f64 {
        let docx = which.of(&self.docx_margins);
        if docx == 0.0 {
            which.of(&self.margins)
        } else {
            docx
        }
    }

    /// Page size minus margins, in points.
    ///
    /// Port of the Python `page_effective_area`.
    pub fn effective_area(&self) -> (f64, f64) {
        let (w, h) = self.size();
        (
            w - self.margin(Edge::Left) - self.margin(Edge::Right),
            h - self.margin(Edge::Top) - self.margin(Edge::Bottom),
        )
    }
}

/// A page edge, used to pick a margin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Left,
    Top,
    Right,
    Bottom,
}

impl Edge {
    /// The order the Python writes `w:pgMar` attributes in.
    pub const ALL: [Edge; 4] = [Edge::Left, Edge::Top, Edge::Right, Edge::Bottom];

    fn of(self, m: &Margins) -> f64 {
        match self {
            Edge::Left => m.left,
            Edge::Top => m.top,
            Edge::Right => m.right,
            Edge::Bottom => m.bottom,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Edge::Left => "left",
            Edge::Top => "top",
            Edge::Right => "right",
            Edge::Bottom => "bottom",
        }
    }
}

/// The empty `document.xml` and `styles.xml` trees a conversion starts
/// from.
///
/// Port of the Python `create_skeleton`, which returns
/// `(doc, styles, body)`; here the body is reached through
/// [`Skeleton::body_mut`] since it is a child of `document`.
#[derive(Debug, Clone)]
pub struct Skeleton {
    pub document: Element,
    pub styles: Element,
}

/// Prefixes declared on `document.xml`, matching the Python's `dn`.
const DOCUMENT_PREFIXES: [&str; 10] = ["w", "r", "m", "ve", "o", "wp", "w10", "wne", "a", "pic"];
/// Prefixes declared on `styles.xml`.
const STYLES_PREFIXES: [&str; 4] = ["w", "r", "a", "wp"];

fn declare(mut e: Element, prefixes: &[&str], ns: &DocxNamespace) -> Element {
    for prefix in prefixes {
        if let Some(uri) = ns.namespace(prefix) {
            e = e.ns(*prefix, uri);
        }
    }
    e
}

/// Build the skeleton for a document with the given page geometry.
///
/// Port of the Python `create_skeleton`.
pub fn create_skeleton(opts: &PageOptions, ns: &DocxNamespace) -> Skeleton {
    let (width, height) = opts.size();
    // Twentieths of a point throughout, truncated as Python's int()
    // does.
    let twips = |pts: f64| (20.0 * pts) as i64;

    let mut document = declare(Element::new("w:document"), &DOCUMENT_PREFIXES, ns);
    let body = document.append(Element::new("w:body"));
    let sect_pr = body.append(Element::new("w:sectPr"));
    sect_pr.append(
        Element::new("w:pgSz")
            .attr("w:w", twips(width).to_string())
            .attr("w:h", twips(height).to_string()),
    );
    let pg_mar = sect_pr.append(Element::new("w:pgMar"));
    for edge in Edge::ALL {
        pg_mar.set(
            format!("w:{}", edge.as_str()),
            twips(opts.margin(edge)).to_string(),
        );
    }
    sect_pr.append(Element::new("w:cols").attr("w:space", "720"));
    sect_pr.append(Element::new("w:docGrid").attr("w:linePitch", "360"));

    let mut styles = declare(Element::new("w:styles"), &STYLES_PREFIXES, ns);
    let doc_defaults = styles.append(Element::new("w:docDefaults"));
    let r_pr = doc_defaults
        .append(Element::new("w:rPrDefault"))
        .append(Element::new("w:rPr"));
    r_pr.append(
        Element::new("w:rFonts")
            .attr("w:asciiTheme", "minorHAnsi")
            .attr("w:eastAsiaTheme", "minorEastAsia")
            .attr("w:hAnsiTheme", "minorHAnsi")
            .attr("w:cstheme", "minorBidi"),
    );
    r_pr.append(Element::new("w:sz").attr("w:val", "22"));
    r_pr.append(Element::new("w:szCs").attr("w:val", "22"));
    r_pr.append(
        Element::new("w:lang")
            .attr("w:val", "en-US")
            .attr("w:eastAsia", "en-US")
            .attr("w:bidi", "ar-SA"),
    );
    doc_defaults
        .append(Element::new("w:pPrDefault"))
        .append(Element::new("w:pPr"))
        .append(
            Element::new("w:spacing")
                .attr("w:after", "0")
                .attr("w:line", "276")
                .attr("w:lineRule", "auto"),
        );

    Skeleton { document, styles }
}

impl Skeleton {
    /// The `w:body` element content is appended to.
    pub fn body_mut(&mut self) -> &mut Element {
        for child in self.document.children.iter_mut() {
            if let super::xml::Child::Element(e) = child {
                if e.name == "w:body" {
                    return e;
                }
            }
        }
        unreachable!("create_skeleton always makes a w:body")
    }
}

/// The relationships of `word/document.xml`.
///
/// Port of the Python `DocumentRelationships`. Ids are handed out in
/// insertion order, so the four boilerplate parts always take `rId1`
/// through `rId4`.
#[derive(Debug, Clone)]
pub struct DocumentRelationships {
    /// `(target, type, target_mode)` → id, in insertion order.
    entries: Vec<((String, String, Option<String>), String)>,
    images_type: String,
}

impl DocumentRelationships {
    pub fn new(ns: &DocxNamespace) -> Self {
        let mut rels = Self {
            entries: Vec::new(),
            images_type: ns.name("IMAGES").unwrap_or_default().to_string(),
        };
        for (key, target) in [
            ("STYLES", "styles.xml"),
            ("NUMBERING", "numbering.xml"),
            ("WEB_SETTINGS", "webSettings.xml"),
            ("FONTS", "fontTable.xml"),
        ] {
            if let Some(typ) = ns.name(key) {
                rels.add(target, typ, None);
            }
        }
        rels
    }

    /// The id of an existing relationship, if there is one.
    ///
    /// Port of the Python `get_relationship_id`.
    pub fn id_of(&self, target: &str, rtype: &str, target_mode: Option<&str>) -> Option<&str> {
        let key = (
            target.to_string(),
            rtype.to_string(),
            target_mode.map(str::to_string),
        );
        self.entries
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, id)| id.as_str())
    }

    /// Add a relationship, or return the existing id for it.
    ///
    /// Port of the Python `add_relationship`.
    pub fn add(&mut self, target: &str, rtype: &str, target_mode: Option<&str>) -> String {
        if let Some(id) = self.id_of(target, rtype, target_mode) {
            return id.to_string();
        }
        let id = format!("rId{}", self.entries.len() + 1);
        self.entries.push((
            (
                target.to_string(),
                rtype.to_string(),
                target_mode.map(str::to_string),
            ),
            id.clone(),
        ));
        id
    }

    /// Port of the Python `add_image`.
    pub fn add_image(&mut self, target: &str) -> String {
        let typ = self.images_type.clone();
        self.add(target, &typ, None)
    }

    /// Serialize to `word/_rels/document.xml.rels`.
    pub fn serialize(&self, ns: &DocxNamespace) -> String {
        let mut root = Element::new("Relationships").ns("", ns.namespace("pr").unwrap_or_default());
        for ((target, rtype, mode), id) in &self.entries {
            let mut r = Element::new("Relationship")
                .attr("Id", id)
                .attr("Type", rtype)
                .attr("Target", target);
            if let Some(mode) = mode {
                r.set("TargetMode", mode);
            }
            root.append(r);
        }
        root.to_xml()
    }
}

/// Content types declared for every package, part name → type.
const PART_CONTENT_TYPES: &[(&str, &str)] = &[
    (
        "/word/footnotes.xml",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml",
    ),
    (
        "/word/document.xml",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
    ),
    (
        "/word/numbering.xml",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml",
    ),
    (
        "/word/styles.xml",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml",
    ),
    (
        "/word/endnotes.xml",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml",
    ),
    (
        "/word/settings.xml",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml",
    ),
    (
        "/word/theme/theme1.xml",
        "application/vnd.openxmlformats-officedocument.theme+xml",
    ),
    (
        "/word/fontTable.xml",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.fontTable+xml",
    ),
    (
        "/word/webSettings.xml",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.webSettings+xml",
    ),
    (
        "/docProps/core.xml",
        "application/vnd.openxmlformats-package.core-properties+xml",
    ),
    (
        "/docProps/app.xml",
        "application/vnd.openxmlformats-officedocument.extended-properties+xml",
    ),
];

/// Extension defaults every package declares.
const EXTENSION_CONTENT_TYPES: &[(&str, &str)] = &[
    ("gif", "image/gif"),
    ("jpeg", "image/jpeg"),
    ("jpg", "image/jpeg"),
    (
        "odttf",
        "application/vnd.openxmlformats-officedocument.obfuscatedFont",
    ),
    ("png", "image/png"),
    (
        "rels",
        "application/vnd.openxmlformats-package.relationships+xml",
    ),
    ("svg", "image/svg+xml"),
    ("xml", "application/xml"),
];

/// A DOCX package under construction.
///
/// Port of the Python writer's `DOCX` class.
pub struct DocxWriter {
    pub namespace: DocxNamespace,
    pub opts: PageOptions,
    pub document_relationships: DocumentRelationships,
    /// `word/fontTable.xml`.
    pub font_table: Element,
    /// `word/numbering.xml`.
    pub numbering: Element,
    /// `word/_rels/fontTable.xml.rels`.
    pub embedded_fonts: Element,
    /// `word/document.xml`.
    pub document: Element,
    /// `word/styles.xml`.
    pub styles: Element,
    /// Extra parts to write verbatim, part name → bytes. Images and
    /// embedded fonts land here.
    pub parts: BTreeMap<String, Vec<u8>>,
    /// The `dcterms` timestamp, as an ISO-8601 string ending in `Z`.
    pub timestamp: String,
    /// Written into `docProps/app.xml`.
    pub application: String,
    pub app_version: String,
}

impl DocxWriter {
    /// Start an empty package.
    pub fn new(opts: PageOptions) -> Self {
        let namespace = DocxNamespace::new(true);
        let skeleton = create_skeleton(&opts, &namespace);
        let mut font_table = Element::new("w:fonts");
        let mut numbering = Element::new("w:numbering");
        for prefix in ["w", "r"] {
            if let Some(uri) = namespace.namespace(prefix) {
                font_table = font_table.ns(prefix, uri);
                numbering = numbering.ns(prefix, uri);
            }
        }
        Self {
            document_relationships: DocumentRelationships::new(&namespace),
            font_table,
            numbering,
            embedded_fonts: Element::new("Relationships")
                .ns("", namespace.namespace("pr").unwrap_or_default()),
            document: skeleton.document,
            styles: skeleton.styles,
            parts: BTreeMap::new(),
            timestamp: "1970-01-01T00:00:00Z".to_string(),
            application: "calibre-oxide".to_string(),
            app_version: "00.0001".to_string(),
            namespace,
            opts,
        }
    }

    /// The `w:body` content is appended to.
    pub fn body_mut(&mut self) -> &mut Element {
        for child in self.document.children.iter_mut() {
            if let super::xml::Child::Element(e) = child {
                if e.name == "w:body" {
                    return e;
                }
            }
        }
        unreachable!("the skeleton always has a w:body")
    }

    /// `[Content_Types].xml`.
    ///
    /// Port of the Python `contenttypes` property. Extensions of parts
    /// added since construction (images, fonts) get a default too.
    pub fn content_types(&self) -> String {
        let mut root =
            Element::new("Types").ns("", self.namespace.namespace("ct").unwrap_or_default());
        for (part, mt) in PART_CONTENT_TYPES {
            root.append(
                Element::new("Override")
                    .attr("PartName", *part)
                    .attr("ContentType", *mt),
            );
        }
        let mut declared: Vec<String> = Vec::new();
        for (ext, mt) in EXTENSION_CONTENT_TYPES {
            declared.push((*ext).to_string());
            root.append(
                Element::new("Default")
                    .attr("Extension", *ext)
                    .attr("ContentType", *mt),
            );
        }
        for name in self.parts.keys() {
            let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
            if ext.is_empty() || declared.contains(&ext) {
                continue;
            }
            if let Some(mt) = mime_guess::from_path(name).first() {
                declared.push(ext.clone());
                root.append(
                    Element::new("Default")
                        .attr("Extension", ext)
                        .attr("ContentType", mt.essence_str()),
                );
            }
        }
        root.to_xml()
    }

    /// `docProps/app.xml`.
    ///
    /// Port of the Python `appproperties` property.
    pub fn app_properties(&self, mi: &MetaInformation) -> String {
        let mut root =
            Element::new("Properties").ns("", self.namespace.namespace("ep").unwrap_or_default());
        for (name, value) in [
            ("Application", self.application.as_str()),
            ("AppVersion", self.app_version.as_str()),
            ("DocSecurity", "0"),
            ("HyperlinksChanged", "false"),
            ("LinksUpToDate", "true"),
            ("ScaleCrop", "false"),
            ("SharedDoc", "false"),
        ] {
            root.append(Element::new(name).with_text(value));
        }
        if let Some(publisher) = &mi.publisher {
            root.append(Element::new("Company").with_text(publisher));
        }
        root.to_xml()
    }

    /// `_rels/.rels`.
    ///
    /// Port of the Python `containerrels` property, including its id
    /// numbering (app is rId3, core rId2, document rId1).
    pub fn container_rels(&self) -> String {
        let ns = &self.namespace;
        let mut root = Element::new("Relationships").ns("", ns.namespace("pr").unwrap_or_default());
        for (id, key, target) in [
            ("rId3", "APPPROPS", "docProps/app.xml"),
            ("rId2", "DOCPROPS", "docProps/core.xml"),
            ("rId1", "DOCUMENT", "word/document.xml"),
        ] {
            root.append(
                Element::new("Relationship")
                    .attr("Id", id)
                    .attr("Type", ns.name(key).unwrap_or_default())
                    .attr("Target", target),
            );
        }
        root.to_xml()
    }

    /// `word/webSettings.xml`.
    ///
    /// Port of the Python `websettings` property.
    pub fn web_settings(&self) -> String {
        let mut root = Element::new("w:webSettings")
            .ns("w", self.namespace.namespace("w").unwrap_or_default());
        for name in [
            "w:optimizeForBrowser",
            "w:allowPNG",
            "w:doNotSaveAsSingleFile",
        ] {
            root.append(Element::new(name));
        }
        root.to_xml()
    }

    /// `docProps/core.xml`.
    ///
    /// Port of the Python `convert_metadata` plus `update_doc_props`.
    /// The language is written as given: calibre canonicalizes it to
    /// ISO-639-1 first, which needs a language database this crate does
    /// not have yet.
    pub fn core_properties(&self, mi: &MetaInformation) -> String {
        let ns = &self.namespace;
        let mut root = Element::new("cp:coreProperties");
        for prefix in ["cp", "dc", "dcterms", "xsi"] {
            if let Some(uri) = ns.namespace(prefix) {
                root = root.ns(prefix, uri);
            }
        }
        root.append(Element::new("cp:revision").with_text("1"));
        root.append(Element::new("cp:lastModifiedBy").with_text("calibre"));
        for name in ["dcterms:created", "dcterms:modified"] {
            root.append(
                Element::new(name)
                    .attr("xsi:type", "dcterms:W3CDTF")
                    .with_text(&self.timestamp),
            );
        }
        root.append(Element::new("dc:title").with_text(&mi.title));
        root.append(Element::new("dc:creator").with_text(authors_to_string(&mi.authors)));
        if !mi.tags.is_empty() {
            root.append(Element::new("cp:keywords").with_text(mi.tags.join(", ")));
        }
        if let Some(comments) = &mi.comments {
            root.append(Element::new("dc:description").with_text(comments));
        }
        if let Some(lang) = mi.languages.first() {
            // Port of `l = canonicalize_lang(mi.languages[0]);
            // setm('language', lang_as_iso639_1(l) or l)`. If `lang`
            // doesn't canonicalize at all (rare -- it's normally
            // already a canonical code by the time it lands in
            // `MetaInformation`), fall back to the raw value rather
            // than writing an empty element.
            let canonical = calibre_utils::localization::canonicalize_lang(lang);
            let text = match &canonical {
                Some(c) => calibre_utils::localization::lang_as_iso639_1(c).unwrap_or_else(|| c.clone()),
                None => lang.clone(),
            };
            root.append(Element::new("dc:language").with_text(&text));
        }
        root.to_xml()
    }

    /// Write the package.
    ///
    /// Port of the Python `write`.
    pub fn write<W: Write + Seek>(&self, sink: W, mi: &MetaInformation) -> Result<(), DocxError> {
        let mut zip = ZipWriter::new(sink);
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        let mut put = |zip: &mut ZipWriter<W>, name: &str, data: &[u8]| -> Result<(), DocxError> {
            zip.start_file(name, options)?;
            zip.write_all(data)?;
            Ok(())
        };

        put(
            &mut zip,
            "[Content_Types].xml",
            self.content_types().as_bytes(),
        )?;
        put(&mut zip, "_rels/.rels", self.container_rels().as_bytes())?;
        put(
            &mut zip,
            "docProps/core.xml",
            self.core_properties(mi).as_bytes(),
        )?;
        put(
            &mut zip,
            "docProps/app.xml",
            self.app_properties(mi).as_bytes(),
        )?;
        put(
            &mut zip,
            "word/webSettings.xml",
            self.web_settings().as_bytes(),
        )?;
        put(
            &mut zip,
            "word/document.xml",
            self.document.to_xml().as_bytes(),
        )?;
        put(&mut zip, "word/styles.xml", self.styles.to_xml().as_bytes())?;
        put(
            &mut zip,
            "word/numbering.xml",
            self.numbering.to_xml().as_bytes(),
        )?;
        put(
            &mut zip,
            "word/fontTable.xml",
            self.font_table.to_xml().as_bytes(),
        )?;
        put(
            &mut zip,
            "word/_rels/document.xml.rels",
            self.document_relationships
                .serialize(&self.namespace)
                .as_bytes(),
        )?;
        put(
            &mut zip,
            "word/_rels/fontTable.xml.rels",
            self.embedded_fonts.to_xml().as_bytes(),
        )?;
        for (name, data) in &self.parts {
            put(&mut zip, name, data)?;
        }
        zip.finish()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docx::container::Docx;
    use roxmltree::Document;
    use std::io::Cursor;

    fn sample_metadata() -> MetaInformation {
        let mut mi = MetaInformation {
            title: "The Moonstone".to_string(),
            authors: vec!["Wilkie Collins".to_string()],
            ..Default::default()
        };
        mi.tags = vec!["mystery".to_string(), "victorian".to_string()];
        mi.comments = Some("The first detective novel.".to_string());
        mi.languages = vec!["en".to_string()];
        mi.publisher = Some("Tinsley Brothers".to_string());
        mi
    }

    #[test]
    fn page_size_prefers_a_custom_size_then_the_named_one() {
        let mut opts = PageOptions::default();
        assert_eq!(opts.size(), (612.0, 792.0), "letter is the default");

        opts.page_size = "a4".to_string();
        assert_eq!(opts.size().0, 595.2755905511812);

        opts.custom_page_size = Some("400x500".to_string());
        assert_eq!(opts.size(), (400.0, 500.0));

        // A malformed custom size falls back rather than failing a
        // conversion that is already under way.
        opts.custom_page_size = Some("wide".to_string());
        assert_eq!(opts.size().0, 595.2755905511812);
        opts.page_size = "nosuchsize".to_string();
        opts.custom_page_size = None;
        assert_eq!(opts.size(), (612.0, 792.0));
    }

    #[test]
    fn a_zero_docx_margin_falls_back_to_the_conversion_margin() {
        let opts = PageOptions {
            docx_margins: Margins {
                left: 0.0,
                top: 36.0,
                right: 0.0,
                bottom: 36.0,
            },
            margins: Margins::uniform(72.0),
            ..Default::default()
        };
        assert_eq!(opts.margin(Edge::Left), 72.0, "zero means unset");
        assert_eq!(opts.margin(Edge::Top), 36.0);
        assert_eq!(opts.effective_area(), (612.0 - 144.0, 792.0 - 72.0));
    }

    #[test]
    fn the_skeleton_carries_the_page_geometry_in_twips() {
        let opts = PageOptions {
            page_size: "a4".to_string(),
            docx_margins: Margins::uniform(36.0),
            ..Default::default()
        };
        let ns = DocxNamespace::new(true);
        let skeleton = create_skeleton(&opts, &ns);
        let xml = skeleton.document.to_xml();
        let doc = Document::parse(&xml).expect("skeleton parses");

        let pg_sz = doc
            .descendants()
            .find(|n| n.tag_name().name() == "pgSz")
            .expect("pgSz");
        // 595.2755... pt * 20, truncated.
        assert_eq!(
            pg_sz.attribute((ns.namespace("w").unwrap(), "w")),
            Some("11905")
        );
        assert_eq!(
            pg_sz.attribute((ns.namespace("w").unwrap(), "h")),
            Some("16837")
        );

        let pg_mar = doc
            .descendants()
            .find(|n| n.tag_name().name() == "pgMar")
            .expect("pgMar");
        for edge in ["left", "top", "right", "bottom"] {
            assert_eq!(
                pg_mar.attribute((ns.namespace("w").unwrap(), edge)),
                Some("720"),
                "{edge}"
            );
        }
    }

    #[test]
    fn the_skeleton_declares_the_prefixes_word_expects() {
        let ns = DocxNamespace::new(true);
        let skeleton = create_skeleton(&PageOptions::default(), &ns);
        let xml = skeleton.document.to_xml();
        for prefix in DOCUMENT_PREFIXES {
            assert!(
                xml.contains(&format!("xmlns:{prefix}=")),
                "missing {prefix}"
            );
        }
        // And styles.xml declares its own, smaller set.
        let sxml = skeleton.styles.to_xml();
        assert!(sxml.contains("xmlns:w="));
        assert!(!sxml.contains("xmlns:pic="));
        assert!(sxml.contains(r#"<w:sz w:val="22"/>"#), "{sxml}");
    }

    #[test]
    fn relationship_ids_are_stable_and_deduplicated() {
        let ns = DocxNamespace::new(true);
        let mut rels = DocumentRelationships::new(&ns);
        // The four boilerplate parts are added first, in order.
        assert_eq!(
            rels.id_of("styles.xml", ns.name("STYLES").unwrap(), None),
            Some("rId1")
        );
        assert_eq!(
            rels.id_of("fontTable.xml", ns.name("FONTS").unwrap(), None),
            Some("rId4")
        );

        let first = rels.add_image("media/image1.png");
        assert_eq!(first, "rId5");
        assert_eq!(rels.add_image("media/image1.png"), "rId5", "deduplicated");
        assert_eq!(rels.add_image("media/image2.png"), "rId6");

        // A different target mode is a different relationship.
        let a = rels.add(
            "https://example.com/",
            ns.name("LINKS").unwrap(),
            Some("External"),
        );
        let b = rels.add("https://example.com/", ns.name("LINKS").unwrap(), None);
        assert_ne!(a, b);

        let xml = rels.serialize(&ns);
        let doc = Document::parse(&xml).expect("parses");
        let external = doc
            .descendants()
            .find(|n| n.attribute("Id") == Some(a.as_str()))
            .unwrap();
        assert_eq!(external.attribute("TargetMode"), Some("External"));
        let internal = doc
            .descendants()
            .find(|n| n.attribute("Id") == Some(b.as_str()))
            .unwrap();
        assert_eq!(internal.attribute("TargetMode"), None);
    }

    #[test]
    fn core_properties_carry_the_book_metadata() {
        let writer = DocxWriter::new(PageOptions::default());
        let xml = writer.core_properties(&sample_metadata());
        let doc = Document::parse(&xml).expect("parses");
        let text_of = |name: &str| -> Option<String> {
            doc.descendants()
                .find(|n| n.tag_name().name() == name)
                .and_then(|n| n.text())
                .map(str::to_string)
        };
        assert_eq!(text_of("title").as_deref(), Some("The Moonstone"));
        assert_eq!(text_of("creator").as_deref(), Some("Wilkie Collins"));
        assert_eq!(text_of("keywords").as_deref(), Some("mystery, victorian"));
        assert_eq!(
            text_of("description").as_deref(),
            Some("The first detective novel.")
        );
        assert_eq!(text_of("language").as_deref(), Some("en"));
        assert_eq!(text_of("created").as_deref(), Some("1970-01-01T00:00:00Z"));
    }

    #[test]
    fn optional_metadata_is_simply_absent() {
        let writer = DocxWriter::new(PageOptions::default());
        let mi = MetaInformation::default();
        let xml = writer.core_properties(&mi);
        assert!(!xml.contains("keywords"), "{xml}");
        assert!(!xml.contains("description"), "{xml}");
        assert!(!writer.app_properties(&mi).contains("Company"));
    }

    #[test]
    fn content_types_declare_added_parts() {
        let mut writer = DocxWriter::new(PageOptions::default());
        writer
            .parts
            .insert("word/media/cover.webp".to_string(), vec![0u8; 4]);
        let xml = writer.content_types();
        assert!(xml.contains(r#"Extension="webp""#), "{xml}");
        // Already-declared extensions are not repeated.
        assert_eq!(xml.matches(r#"Extension="png""#).count(), 1);
    }

    /// The round trip: a package written here must be readable by the
    /// reader ported in #22, which was written from the same spec but
    /// independently of this code.
    #[test]
    fn a_written_package_reads_back_through_the_docx_reader() {
        let mi = sample_metadata();
        let mut writer = DocxWriter::new(PageOptions {
            page_size: "a4".to_string(),
            ..Default::default()
        });
        // A paragraph of real content, so document.xml is not trivial.
        {
            let body = writer.body_mut();
            let p = body.append(Element::new("w:p"));
            p.append(Element::new("w:r"))
                .append(Element::new("w:t").with_text("Hello & goodbye"));
        }
        writer
            .parts
            .insert("word/media/image1.png".to_string(), b"\x89PNG".to_vec());
        writer.document_relationships.add_image("media/image1.png");

        let mut buf = Cursor::new(Vec::new());
        writer.write(&mut buf, &mi).expect("writes");
        buf.set_position(0);

        let mut docx = Docx::new(buf).expect("the reader accepts the package");
        assert!(docx.is_transitional());
        assert_eq!(docx.document_name().unwrap(), "word/document.xml");
        assert_eq!(
            docx.content_type("word/document.xml").as_deref(),
            Some(
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"
            )
        );
        assert_eq!(
            docx.content_type("word/media/image1.png").as_deref(),
            Some("image/png")
        );

        let rels = docx.document_relationships().unwrap();
        assert_eq!(rels.target("rId5"), Some("word/media/image1.png"));

        let read_mi = docx.metadata();
        assert_eq!(read_mi.title, "The Moonstone");
        assert_eq!(read_mi.authors, vec!["Wilkie Collins"]);
        assert_eq!(read_mi.publisher.as_deref(), Some("Tinsley Brothers"));
        // "en" canonicalizes to "eng" on the way in (core_properties
        // narrows to ISO 639-1 for the written dc:language text, and
        // the reader canonicalizes back to calibre's own 3-letter
        // convention on the way out) -- see calibre_utils::localization.
        assert_eq!(read_mi.languages, vec!["eng"]);
        assert_eq!(read_mi.tags, vec!["mystery", "victorian"]);

        // And the body survived, escaping included.
        let body = docx.read_str("word/document.xml").unwrap();
        let parsed = Document::parse(&body).expect("document.xml parses");
        let text: String = parsed
            .descendants()
            .filter(|n| n.is_text())
            .filter_map(|n| n.text())
            .collect();
        assert_eq!(text, "Hello & goodbye");
        assert_eq!(docx.read("word/media/image1.png").unwrap(), b"\x89PNG");
    }
}
