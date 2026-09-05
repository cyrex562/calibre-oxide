//! The DOCX package: the OPC zip container, its content types,
//! relationships, and document properties.
//!
//! Port of `old_src/src/calibre/ebooks/docx/container.py`.
//!
//! One deliberate deviation: the Python class can either read parts
//! straight out of the zip or extract the whole package to a temporary
//! directory first, the latter existing so a fallback "forgiving" zip
//! parser can be swapped in for damaged files. This port always reads
//! from the archive — the `zip` crate already reports damage as an
//! error rather than crashing — and offers [`Docx::extract_to`] for
//! callers that genuinely want the files on disk, such as
//! [`super::dump`].

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::path::{Component, Path};

use roxmltree::Document;
use zip::ZipArchive;

use super::error::DocxError;
use super::names::{DocxNamespace, STRICT_DOCUMENT_RELATIONSHIP};
use crate::metadata::authors::{authors_to_sort_string, string_to_authors};
use crate::metadata::meta::MetaInformation;

/// The relationships declared by one part, indexed both ways.
///
/// Port of the `(by_id, by_type)` pair the Python `get_relationships`
/// returns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Relationships {
    /// `Id` → target part name.
    pub by_id: HashMap<String, String>,
    /// Relationship type → target part name.
    pub by_type: HashMap<String, String>,
}

impl Relationships {
    /// The target of a relationship id, as written in the document
    /// body (`r:embed="rId4"`).
    pub fn target(&self, id: &str) -> Option<&str> {
        self.by_id.get(id).map(String::as_str)
    }
}

/// An open DOCX package.
///
/// Port of the Python `DOCX` class.
pub struct Docx<R: Read + Seek> {
    zip: ZipArchive<R>,
    /// Every part name in the package.
    names: HashSet<String>,
    /// Per-part content type overrides, keyed by part name.
    pub content_types: HashMap<String, String>,
    /// Per-extension default content types, keyed by lowercase
    /// extension.
    pub default_content_types: HashMap<String, String>,
    /// Package-level relationships: type → target.
    pub relationships: HashMap<String, String>,
    /// Package-level relationships: target → type.
    pub relationships_rmap: HashMap<String, String>,
    /// The namespace table for this package's OOXML flavour.
    pub namespace: DocxNamespace,
}

impl<R: Read + Seek> std::fmt::Debug for Docx<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Docx")
            .field("parts", &self.names.len())
            .field("transitional", &self.namespace.transitional)
            .finish_non_exhaustive()
    }
}

impl<R: Read + Seek> Docx<R> {
    /// Open a package from any seekable reader.
    pub fn new(reader: R) -> Result<Self, DocxError> {
        let zip = ZipArchive::new(reader)?;
        let names: HashSet<String> = zip.file_names().map(str::to_string).collect();
        let mut docx = Self {
            zip,
            names,
            content_types: HashMap::new(),
            default_content_types: HashMap::new(),
            relationships: HashMap::new(),
            relationships_rmap: HashMap::new(),
            namespace: DocxNamespace::new(true),
        };
        docx.read_content_types()?;
        let transitional = docx.read_package_relationships()?;
        docx.namespace = DocxNamespace::new(transitional);
        Ok(docx)
    }

    /// Whether the package uses the transitional (Word-authored)
    /// vocabulary rather than ISO Strict.
    pub fn is_transitional(&self) -> bool {
        self.namespace.transitional
    }

    /// Whether a part exists.
    pub fn exists(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    /// Every part name in the package.
    pub fn names(&self) -> &HashSet<String> {
        &self.names
    }

    /// Read a part's bytes.
    pub fn read(&mut self, name: &str) -> Result<Vec<u8>, DocxError> {
        let mut file = self
            .zip
            .by_name(name)
            .map_err(|_| DocxError::MissingPart(name.to_string()))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        Ok(buf)
    }

    /// Read a part as text. Invalid UTF-8 is replaced rather than
    /// rejected: a single bad byte in a comment should not cost the
    /// reader the whole document.
    pub fn read_str(&mut self, name: &str) -> Result<String, DocxError> {
        Ok(String::from_utf8_lossy(&self.read(name)?).into_owned())
    }

    /// The content type of a part: its override, else its extension's
    /// default, else a guess from the file name.
    ///
    /// Port of the Python `content_type`.
    pub fn content_type(&self, name: &str) -> Option<String> {
        if let Some(ct) = self.content_types.get(name) {
            return Some(ct.clone());
        }
        let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
        if let Some(ct) = self.default_content_types.get(&ext) {
            return Some(ct.clone());
        }
        mime_guess::from_path(name)
            .first()
            .map(|m| m.essence_str().to_string())
    }

    /// Port of the Python `read_content_types`.
    fn read_content_types(&mut self) -> Result<(), DocxError> {
        let raw = self
            .read_str("[Content_Types].xml")
            .map_err(|_| DocxError::InvalidDocx("no [Content_Types].xml".into()))?;
        let doc = Document::parse(&raw)?;
        // Matched by local name only: the content-types part is one of
        // the few whose namespace differs between producers.
        for item in doc.descendants().filter(|n| n.is_element()) {
            match item.tag_name().name() {
                "Default" => {
                    if let (Some(ext), Some(ct)) =
                        (item.attribute("Extension"), item.attribute("ContentType"))
                    {
                        self.default_content_types
                            .insert(ext.to_lowercase(), ct.to_string());
                    }
                }
                "Override" => {
                    if let (Some(part), Some(ct)) =
                        (item.attribute("PartName"), item.attribute("ContentType"))
                    {
                        self.content_types
                            .insert(part.trim_start_matches('/').to_string(), ct.to_string());
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Read `_rels/.rels`, returning whether the package is
    /// transitional.
    ///
    /// Port of the Python `read_package_relationships`, including the
    /// flavour detection: a package is strict when the relationship
    /// pointing at `word/document.xml` uses the strict type URI.
    fn read_package_relationships(&mut self) -> Result<bool, DocxError> {
        let raw = self
            .read_str("_rels/.rels")
            .map_err(|_| DocxError::InvalidDocx("no _rels/.rels".into()))?;
        let doc = Document::parse(&raw)?;
        let mut transitional = true;
        for item in doc
            .descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "Relationship")
        {
            let (Some(typ), Some(target)) = (item.attribute("Type"), item.attribute("Target"))
            else {
                continue;
            };
            let target = target.trim_start_matches('/').to_string();
            if target == "word/document.xml" {
                transitional = typ != STRICT_DOCUMENT_RELATIONSHIP;
            }
            self.relationships.insert(typ.to_string(), target.clone());
            self.relationships_rmap.insert(target, typ.to_string());
        }
        Ok(transitional)
    }

    /// The name of the main document part.
    ///
    /// Port of the Python `document_name`.
    pub fn document_name(&self) -> Result<String, DocxError> {
        if let Some(name) = self
            .namespace
            .name("DOCUMENT")
            .and_then(|t| self.relationships.get(t))
        {
            return Ok(name.clone());
        }
        // Some producers omit the relationship; fall back to the
        // conventional location. Sorted so the choice is deterministic
        // when several candidates exist.
        let mut candidates: Vec<&String> = self
            .names
            .iter()
            .filter(|n| *n == "document.xml" || n.ends_with("/document.xml"))
            .collect();
        candidates.sort();
        candidates
            .first()
            .map(|n| (*n).clone())
            .ok_or_else(|| DocxError::InvalidDocx("no main document part".into()))
    }

    /// The relationships of a named part, read from its `_rels`
    /// sidecar. A part with no sidecar has no relationships, which is
    /// not an error.
    ///
    /// Port of the Python `get_relationships`.
    pub fn get_relationships(&mut self, name: &str) -> Relationships {
        let mut rels = Relationships::default();
        let (base, file) = match name.rfind('/') {
            Some(i) => (&name[..i], &name[i + 1..]),
            None => ("", name),
        };
        let rels_name = if base.is_empty() {
            format!("_rels/{file}.rels")
        } else {
            format!("{base}/_rels/{file}.rels")
        };
        let Ok(raw) = self.read_str(&rels_name) else {
            return rels;
        };
        let Ok(doc) = Document::parse(&raw) else {
            return rels;
        };
        for item in doc
            .descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "Relationship")
        {
            let (Some(typ), Some(target)) = (item.attribute("Type"), item.attribute("Target"))
            else {
                continue;
            };
            // External targets and in-document fragments are used
            // verbatim; a target starting with `/` is absolute from
            // the package root (OPC §, resolved without the part's own
            // directory); everything else is relative to that
            // directory. Issue #139 (#3): a prior version stripped the
            // leading `/` but still joined onto `base` regardless, so
            // an absolute target like `/word/styles.xml` inside `word/`
            // resolved to `word/word/styles.xml` -- a real bug, not a
            // reproduced calibre quirk (Word itself never writes
            // absolute targets, so this was latent, but any producer
            // that does would have its parts silently not found).
            let resolved =
                if item.attribute("TargetMode") == Some("External") || target.starts_with('#') {
                    target.to_string()
                } else if let Some(absolute) = target.strip_prefix('/') {
                    absolute.to_string()
                } else if base.is_empty() {
                    target.to_string()
                } else {
                    format!("{base}/{target}")
                };
            if let Some(id) = item.attribute("Id") {
                rels.by_id.insert(id.to_string(), resolved.clone());
            }
            rels.by_type.insert(typ.to_string(), resolved);
        }
        rels
    }

    /// The main document's relationships.
    pub fn document_relationships(&mut self) -> Result<Relationships, DocxError> {
        let name = self.document_name()?;
        Ok(self.get_relationships(&name))
    }

    /// The names of the core and extended document-property parts.
    ///
    /// Port of the Python `get_document_properties_names`.
    pub fn document_properties_names(&self) -> (Option<String>, Option<String>) {
        let find = |key: &str, conventional: &str| -> Option<String> {
            if let Some(name) = self
                .namespace
                .name(key)
                .and_then(|t| self.relationships.get(t))
            {
                return Some(name.clone());
            }
            self.names
                .iter()
                .find(|n| n.to_lowercase() == conventional)
                .cloned()
        };
        (
            find("DOCPROPS", "docprops/core.xml"),
            find("APPPROPS", "docprops/app.xml"),
        )
    }

    /// Book metadata read from the package's document properties.
    ///
    /// Port of the Python `DOCX.metadata`.
    pub fn metadata(&mut self) -> MetaInformation {
        let mut mi = MetaInformation::default();
        let (core, app) = self.document_properties_names();

        if let Some(name) = core {
            if let Ok(raw) = self.read_str(&name) {
                read_doc_props(&raw, &mut mi, &self.namespace);
            }
        }
        if mi.languages.iter().all(|l| l.is_empty() || l == "und") {
            // Word does not always write dc:language; the default run
            // properties carry it too.
            if let Ok(raw) = self.read_str("word/styles.xml") {
                read_default_style_language(&raw, &mut mi, &self.namespace);
            }
        }
        if let Some(name) = app {
            if let Ok(raw) = self.read_str(&name) {
                read_app_props(&raw, &mut mi);
            }
        }
        mi
    }

    /// Extract every part to `dest`, creating directories as needed.
    ///
    /// Entries whose names would escape `dest` (absolute paths, `..`
    /// segments) are skipped rather than written — a malformed or
    /// hostile package must not be able to write outside the directory
    /// it was pointed at.
    pub fn extract_to(&mut self, dest: &Path) -> Result<Vec<String>, DocxError> {
        std::fs::create_dir_all(dest)?;
        let mut written = Vec::new();
        for i in 0..self.zip.len() {
            let mut entry = self.zip.by_index(i)?;
            // `enclosed_name` borrows the entry, so take an owned copy
            // before reading the entry's bytes.
            let Some(rel) = entry.enclosed_name().map(|p| p.to_path_buf()) else {
                continue;
            };
            if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
                continue;
            }
            let out = dest.join(&rel);
            if entry.is_dir() {
                std::fs::create_dir_all(&out)?;
                continue;
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            std::fs::write(&out, buf)?;
            written.push(rel.to_string_lossy().replace('\\', "/"));
        }
        written.sort();
        Ok(written)
    }
}

impl Docx<std::fs::File> {
    /// Open a package from a path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DocxError> {
        Self::new(std::fs::File::open(path)?)
    }
}

/// Read `docProps/core.xml` into book metadata.
///
/// Port of the Python `read_doc_props`.
pub fn read_doc_props(raw: &str, mi: &mut MetaInformation, ns: &DocxNamespace) {
    let Ok(doc) = Document::parse(raw) else {
        return;
    };
    let root = doc.root_element();
    // `descendants` yields the node itself first, and `Node::text` on an
    // element returns its first text child — so filtering to text nodes
    // is what avoids counting the same run of characters twice.
    let text_of = |n: roxmltree::Node| -> Option<String> {
        let t: String = n
            .descendants()
            .filter(|d| d.is_text())
            .filter_map(|d| d.text())
            .collect();
        let t = t.trim().to_string();
        (!t.is_empty()).then_some(t)
    };

    if let Some(title) = ns
        .descendants(root, &["dc:title"])
        .into_iter()
        .find_map(text_of)
    {
        mi.title = title;
    }

    let mut tags = Vec::new();
    for subject in ns.descendants(root, &["dc:subject"]) {
        if let Some(t) = text_of(subject) {
            // Commas separate calibre tags, so a subject containing one
            // has to be protected.
            tags.push(t.replace(',', "_"));
        }
    }
    for keywords in ns.descendants(root, &["cp:keywords"]) {
        if let Some(t) = text_of(keywords) {
            for chunk in t.split_whitespace() {
                tags.extend(
                    chunk
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                );
            }
        }
    }
    if !tags.is_empty() {
        mi.tags = tags;
    }

    let mut authors = Vec::new();
    for creator in ns.descendants(root, &["dc:creator"]) {
        if let Some(t) = text_of(creator) {
            authors.extend(string_to_authors(&t));
        }
    }
    if !authors.is_empty() {
        mi.author_sort = Some(authors_to_sort_string(&authors));
        mi.authors = authors;
    }

    if let Some(desc) = ns.descendants(root, &["dc:description"]).into_iter().next() {
        let text: String = desc
            .descendants()
            .filter(|d| d.is_text())
            .filter_map(|d| d.text())
            .collect();
        // Word 2007 mangles newlines in the summary into this literal.
        let text = text.replace("_x000d_", "");
        let text = text.trim();
        if !text.is_empty() {
            mi.comments = Some(text.to_string());
        }
    }

    let langs: Vec<String> = ns
        .descendants(root, &["dc:language"])
        .into_iter()
        .filter_map(text_of)
        .filter_map(|t| calibre_utils::localization::canonicalize_lang(&t))
        .collect();
    if !langs.is_empty() {
        mi.languages = langs;
    }
}

/// Read `docProps/app.xml` for the publisher.
///
/// Port of the Python `read_app_props`.
pub fn read_app_props(raw: &str, mi: &mut MetaInformation) {
    let Ok(doc) = Document::parse(raw) else {
        return;
    };
    // Matched by local name: the extended-properties namespace differs
    // between the transitional and strict flavours.
    if let Some(company) = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "Company")
    {
        if let Some(text) = company.text().map(str::trim).filter(|t| !t.is_empty()) {
            mi.publisher = Some(text.to_string());
        }
    }
}

/// Fall back to the document's default run language.
///
/// Port of the Python `read_default_style_language`.
pub fn read_default_style_language(raw: &str, mi: &mut MetaInformation, ns: &DocxNamespace) {
    let Ok(doc) = Document::parse(raw) else {
        return;
    };
    let root = doc.root_element();
    if !ns.is_tag(root, "w:styles") {
        return;
    }
    for defaults in ns.children(root, &["w:docDefaults"]) {
        for rpr_default in ns.children(defaults, &["w:rPrDefault"]) {
            for rpr in ns.children(rpr_default, &["w:rPr"]) {
                for lang in ns.children(rpr, &["w:lang"]) {
                    if let Some(canonical) = ns
                        .get(lang, "w:val")
                        .map(str::trim)
                        .filter(|v| !v.is_empty())
                        .and_then(calibre_utils::localization::canonicalize_lang)
                    {
                        mi.languages = vec![canonical];
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::FileOptions;

    /// Build a minimal but well-formed DOCX package.
    fn package(parts: &[(&str, &str)]) -> Cursor<Vec<u8>> {
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
            let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
            for (name, content) in parts {
                zip.start_file(*name, options).unwrap();
                zip.write_all(content.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }
        Cursor::new(buf)
    }

    const CONTENT_TYPES: &str = r#"<?xml version="1.0"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="PNG" ContentType="image/png"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#;

    const RELS: &str = r#"<?xml version="1.0"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>
  <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/>
</Relationships>"#;

    const DOC_RELS: &str = r#"<?xml version="1.0"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>
  <Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/" TargetMode="External"/>
  <Relationship Id="rId6" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="/word/styles.xml"/>
</Relationships>"#;

    const CORE_PROPS: &str = r#"<?xml version="1.0"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
                   xmlns:dc="http://purl.org/dc/elements/1.1/">
  <dc:title>  A Study in Scarlet  </dc:title>
  <dc:creator>Arthur Conan Doyle &amp; Someone Else</dc:creator>
  <dc:subject>detective, fiction</dc:subject>
  <cp:keywords>holmes watson,london</cp:keywords>
  <dc:description>First outing._x000d_Second line.</dc:description>
  <dc:language>en-GB</dc:language>
</cp:coreProperties>"#;

    const APP_PROPS: &str = r#"<?xml version="1.0"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties">
  <Company>Ward, Lock &amp; Co.</Company>
</Properties>"#;

    fn full_package() -> Docx<Cursor<Vec<u8>>> {
        Docx::new(package(&[
            ("[Content_Types].xml", CONTENT_TYPES),
            ("_rels/.rels", RELS),
            ("word/document.xml", "<w:document/>"),
            ("word/_rels/document.xml.rels", DOC_RELS),
            ("docProps/core.xml", CORE_PROPS),
            ("docProps/app.xml", APP_PROPS),
        ]))
        .expect("opens")
    }

    #[test]
    fn a_package_without_content_types_is_rejected() {
        let err = Docx::new(package(&[("_rels/.rels", RELS)])).unwrap_err();
        assert!(
            matches!(&err, DocxError::InvalidDocx(m) if m.contains("[Content_Types].xml")),
            "got {err:?}"
        );
    }

    #[test]
    fn a_package_without_package_relationships_is_rejected() {
        let err = Docx::new(package(&[("[Content_Types].xml", CONTENT_TYPES)])).unwrap_err();
        assert!(
            matches!(&err, DocxError::InvalidDocx(m) if m.contains("_rels/.rels")),
            "got {err:?}"
        );
    }

    #[test]
    fn something_that_is_not_a_zip_is_rejected() {
        let err = Docx::new(Cursor::new(b"not a zip file at all".to_vec())).unwrap_err();
        assert!(matches!(err, DocxError::Zip(_)), "got {err:?}");
    }

    #[test]
    fn content_types_prefer_override_then_extension_then_guess() {
        let docx = full_package();
        assert_eq!(
            docx.content_type("word/document.xml").as_deref(),
            Some(
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"
            )
        );
        // Extension defaults are matched case-insensitively.
        assert_eq!(
            docx.content_type("word/media/image1.png").as_deref(),
            Some("image/png")
        );
        // Nothing declared: fall back to guessing from the name.
        assert_eq!(
            docx.content_type("word/media/photo.jpeg").as_deref(),
            Some("image/jpeg")
        );
    }

    #[test]
    fn the_main_document_is_found_through_the_relationship() {
        let docx = full_package();
        assert_eq!(docx.document_name().unwrap(), "word/document.xml");
        assert!(docx.is_transitional());
    }

    #[test]
    fn a_missing_document_relationship_falls_back_to_the_conventional_name() {
        let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>"#;
        let docx = Docx::new(package(&[
            ("[Content_Types].xml", CONTENT_TYPES),
            ("_rels/.rels", rels),
            ("word/document.xml", "<w:document/>"),
        ]))
        .unwrap();
        assert_eq!(docx.document_name().unwrap(), "word/document.xml");
    }

    #[test]
    fn a_package_with_no_document_at_all_reports_it() {
        let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>"#;
        let docx = Docx::new(package(&[
            ("[Content_Types].xml", CONTENT_TYPES),
            ("_rels/.rels", rels),
        ]))
        .unwrap();
        assert!(matches!(
            docx.document_name(),
            Err(DocxError::InvalidDocx(_))
        ));
    }

    #[test]
    fn a_strict_package_selects_the_strict_namespace_table() {
        let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;
        let docx = Docx::new(package(&[
            ("[Content_Types].xml", CONTENT_TYPES),
            ("_rels/.rels", rels),
            ("word/document.xml", "<w:document/>"),
        ]))
        .unwrap();
        assert!(!docx.is_transitional());
        assert_eq!(
            docx.namespace.namespace("w"),
            Some("http://purl.oclc.org/ooxml/wordprocessingml/main")
        );
        assert_eq!(docx.document_name().unwrap(), "word/document.xml");
    }

    #[test]
    fn part_relationships_resolve_against_the_parts_directory() {
        let mut docx = full_package();
        let rels = docx.document_relationships().unwrap();
        // Relative internal target, resolved against `word/`.
        assert_eq!(rels.target("rId4"), Some("word/media/image1.png"));
        // External target, left alone.
        assert_eq!(rels.target("rId5"), Some("https://example.com/"));
        // An absolute internal target (`/word/styles.xml`) resolves
        // from the package root, not against the part's own directory
        // -- issue #139 (#3) fixed a real bug where the leading slash
        // was stripped but the result was still joined onto `base`
        // anyway, giving `word/word/styles.xml`.
        assert_eq!(rels.target("rId6"), Some("word/styles.xml"));
        assert_eq!(rels.target("nosuch"), None);
    }

    #[test]
    fn a_part_without_a_rels_sidecar_has_no_relationships() {
        let mut docx = full_package();
        assert_eq!(
            docx.get_relationships("word/styles.xml"),
            Relationships::default()
        );
    }

    #[test]
    fn metadata_is_read_from_the_document_properties() {
        let mut docx = full_package();
        let mi = docx.metadata();
        assert_eq!(mi.title, "A Study in Scarlet");
        assert_eq!(mi.authors, vec!["Arthur Conan Doyle", "Someone Else"]);
        assert!(mi.author_sort.is_some());
        assert_eq!(mi.publisher.as_deref(), Some("Ward, Lock & Co."));
        assert_eq!(mi.languages, vec!["eng"]);
        // The Word 2007 newline mangling is undone.
        assert_eq!(mi.comments.as_deref(), Some("First outing.Second line."));
        // A comma inside a subject would split one tag into two, so it
        // is protected; keywords split on both commas and whitespace.
        assert_eq!(
            mi.tags,
            vec!["detective_ fiction", "holmes", "watson", "london"]
        );
    }

    #[test]
    fn language_falls_back_to_the_default_run_properties() {
        const STYLES: &str = r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:docDefaults><w:rPrDefault><w:rPr><w:lang w:val="fr-FR"/></w:rPr></w:rPrDefault></w:docDefaults>
</w:styles>"#;
        let core = r#"<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
                        xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>T</dc:title></cp:coreProperties>"#;
        let mut docx = Docx::new(package(&[
            ("[Content_Types].xml", CONTENT_TYPES),
            ("_rels/.rels", RELS),
            ("word/document.xml", "<w:document/>"),
            ("word/styles.xml", STYLES),
            ("docProps/core.xml", core),
        ]))
        .unwrap();
        let mi = docx.metadata();
        assert_eq!(mi.languages, vec!["fra"]);
    }

    #[test]
    fn metadata_survives_unparseable_property_parts() {
        let mut docx = Docx::new(package(&[
            ("[Content_Types].xml", CONTENT_TYPES),
            ("_rels/.rels", RELS),
            ("word/document.xml", "<w:document/>"),
            ("docProps/core.xml", "<not xml at <all>"),
        ]))
        .unwrap();
        let mi = docx.metadata();
        assert_eq!(mi.title, "Unknown", "falls back to the default title");
    }

    #[test]
    fn extraction_writes_every_part_under_the_destination() {
        let dir = tempfile::tempdir().unwrap();
        let mut docx = full_package();
        let written = docx.extract_to(dir.path()).unwrap();
        assert!(written.contains(&"word/document.xml".to_string()));
        assert!(dir.path().join("word/_rels/document.xml.rels").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("word/document.xml")).unwrap(),
            "<w:document/>"
        );
    }

    #[test]
    fn extraction_refuses_to_escape_the_destination() {
        // A zip-slip entry must be skipped, not written above `dest`.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("out");
        let mut docx = Docx::new(package(&[
            ("[Content_Types].xml", CONTENT_TYPES),
            ("_rels/.rels", RELS),
            ("../escaped.txt", "nope"),
        ]))
        .unwrap();
        let written = docx.extract_to(&root).unwrap();
        assert!(
            !written.iter().any(|w| w.contains("escaped")),
            "written: {written:?}"
        );
        assert!(!dir.path().join("escaped.txt").exists());
    }

    #[test]
    fn reading_a_missing_part_names_it() {
        let mut docx = full_package();
        assert!(docx.exists("word/document.xml"));
        assert!(!docx.exists("word/footnotes.xml"));
        let err = docx.read("word/footnotes.xml").unwrap_err();
        assert!(
            matches!(&err, DocxError::MissingPart(n) if n == "word/footnotes.xml"),
            "got {err:?}"
        );
    }
}
