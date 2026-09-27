use crate::metadata::zip_edit::placeholders;
use crate::metadata::MetaInformation;
use crate::xmltree::{Xml, XmlNodeId};
use anyhow::{Context, Result};
use std::io::{Read, Seek};
use std::path::Path;
use zip::ZipArchive;

pub fn get_metadata<R: Read + Seek>(stream: R) -> Result<MetaInformation> {
    let mut archive = ZipArchive::new(stream).context("Failed to open DOCX archive")?;
    let mut mi = MetaInformation::default();

    // 1. docProps/core.xml (DC Metadata)
    if let Ok(mut file) = archive.by_name("docProps/core.xml") {
        let mut xml = String::new();
        file.read_to_string(&mut xml)?;
        // Simple XML parsing using roxmltree
        if let Ok(doc) = roxmltree::Document::parse(&xml) {
            for node in doc.descendants() {
                match node.tag_name().name() {
                    "title" => {
                        if let Some(t) = node.text() {
                            mi.title = t.to_string();
                        }
                    }
                    "creator" => {
                        if let Some(t) = node.text() {
                            mi.authors = vec![t.to_string()];
                        }
                    } // Split by comma?
                    "description" => {
                        if let Some(t) = node.text() {
                            mi.comments = Some(t.to_string());
                        }
                    }
                    "subject" => {
                        if let Some(t) = node.text() {
                            mi.tags = t
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                        }
                    }
                    "keywords" => {
                        if let Some(t) = node.text() {
                            let kw = t
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect::<Vec<_>>();
                            mi.tags.extend(kw);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // 2. docProps/app.xml (Publisher)
    if let Ok(mut file) = archive.by_name("docProps/app.xml") {
        let mut xml = String::new();
        file.read_to_string(&mut xml)?;
        if let Ok(doc) = roxmltree::Document::parse(&xml) {
            if let Some(company) = doc.descendants().find(|n| n.tag_name().name() == "Company") {
                if let Some(t) = company.text() {
                    mi.publisher = Some(t.to_string());
                }
            }
        }
    }

    // 3. Cover (docProps/thumbnail.jpeg or similar)
    // Priority: docProps/thumbnail.jpeg, .jpg, .png
    let candidates = [
        "docProps/thumbnail.jpeg",
        "docProps/thumbnail.jpg",
        "docProps/thumbnail.png",
    ];
    for cand in candidates {
        if let Ok(mut file) = archive.by_name(cand) {
            let mut data = Vec::new();
            file.read_to_end(&mut data)?;
            let ext = std::path::Path::new(cand)
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("jpg")
                .to_string();
            mi.cover_data = (Some(ext), data);
            break;
        }
    }

    Ok(mi)
}

const DC_NS: &str = "http://purl.org/dc/elements/1.1/";
const CP_NS: &str = "http://schemas.openxmlformats.org/package/2006/metadata/core-properties";
const EP_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties";

const CORE_PROPS: &str = "docProps/core.xml";
const APP_PROPS: &str = "docProps/app.xml";

/// Writes `mi` into a DOCX's document properties, in place (#834).
///
/// Port of `metadata/docx.py`'s `set_metadata` plus
/// `docx/writer/container.py`'s `update_doc_props`. DOCX splits its
/// metadata across two parts: `docProps/core.xml` holds the Dublin Core
/// fields, and `docProps/app.xml` holds `Company`, which is where a
/// publisher lives.
///
/// **One deliberate divergence from upstream:** `update_doc_props` sets
/// title and creator unconditionally, so passing it a `Metadata` with
/// calibre's own "Unknown" placeholders writes those over real values.
/// This honours [`crate::metadata::zip_edit::placeholders`] instead -- see
/// the EPUB/ODT writers, where the same bug was found and fixed.
pub fn set_metadata(path: &Path, mi: &MetaInformation) -> Result<()> {
    let core = crate::metadata::zip_edit::read_entry_text(path, CORE_PROPS)
        .with_context(|| format!("{CORE_PROPS} is missing -- this does not look like a Word document"))?;
    crate::metadata::zip_edit::replace_entry(path, CORE_PROPS, &rewrite_core_props(&core, mi)?)?;

    // `app.xml` is optional, and its absence is not an error -- upstream
    // reads it in a `try` and simply skips the replacement when it is not
    // there. Only the publisher lives in it, so there is nothing to do
    // when no publisher is being set either.
    if let Some(publisher) = mi.publisher.as_ref().filter(|p| !p.trim().is_empty()) {
        if let Ok(app) = crate::metadata::zip_edit::read_entry_text(path, APP_PROPS) {
            let updated = rewrite_app_props(&app, publisher)?;
            crate::metadata::zip_edit::replace_entry(path, APP_PROPS, &updated)?;
        }
    }
    Ok(())
}

/// Replaces every `<ns:local>` child of `root` with a single one holding
/// `text` -- the shape of upstream's own `setm`.
fn set_single(xml: &mut Xml, root: XmlNodeId, ns: &str, local: &str, text: &str) {
    for child in xml.element_children(root) {
        if xml.namespace(child) == Some(ns) && xml.local_name(child) == Some(local) {
            xml.detach(child);
        }
    }
    let element = xml.new_element(local, Some(ns));
    xml.set_element_text(element, text);
    xml.insert_element(root, element, None);
}

fn rewrite_core_props(core: &str, mi: &MetaInformation) -> Result<Vec<u8>> {
    let mut xml = Xml::parse(core).context("parsing docProps/core.xml")?;
    let root = xml.root_element().context("docProps/core.xml has no root element")?;

    xml.ensure_namespace_declared(Some("dc"), DC_NS);
    xml.ensure_namespace_declared(Some("cp"), CP_NS);

    if let Some(title) = placeholders::real_title(&mi.title) {
        set_single(&mut xml, root, DC_NS, "title", title);
    }
    if let Some(authors) = placeholders::real_authors(&mi.authors) {
        // `dc:creator` is one string here, as in ODT -- joined the way the
        // reader splits it back out.
        set_single(&mut xml, root, DC_NS, "creator", &authors.join(" & "));
    }
    if !mi.tags.is_empty() {
        // `cp:keywords`, comma-joined, matching upstream. Note `dc:subject`
        // is deliberately left alone: it is a single subject rather than a
        // tag list, and clearing it would delete something the user wrote.
        // The reader surfaces both as tags, so a document with a subject
        // shows it alongside these.
        set_single(&mut xml, root, CP_NS, "keywords", &mi.tags.join(", "));
    }
    if let Some(comments) = mi.comments.as_ref().filter(|c| !c.trim().is_empty()) {
        set_single(&mut xml, root, DC_NS, "description", comments);
    }
    if let Some(languages) = placeholders::real_languages(&mi.languages) {
        set_single(&mut xml, root, DC_NS, "language", &languages[0]);
    }
    Ok(xml.serialize())
}

fn rewrite_app_props(app: &str, publisher: &str) -> Result<Vec<u8>> {
    let mut xml = Xml::parse(app).context("parsing docProps/app.xml")?;
    let root = xml.root_element().context("docProps/app.xml has no root element")?;
    // `app.xml` puts its elements in the extended-properties namespace as
    // the *default*, so `Company` is unprefixed there.
    xml.ensure_namespace_declared(None, EP_NS);
    set_single(&mut xml, root, EP_NS, "Company", publisher);
    Ok(xml.serialize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::io::Write;
    use zip::write::FileOptions;

    #[test]
    fn test_docx_metadata() -> Result<()> {
        let mut buffer = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buffer));
            let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);

            // docProps/core.xml
            zip.start_file("docProps/core.xml", options)?;
            let core = r#"
            <cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/">
                <dc:title>Test Document</dc:title>
                <dc:creator>John Doe</dc:creator>
                <dc:description>A test document.</dc:description>
                <cp:keywords>tag1, tag2</cp:keywords>
            </cp:coreProperties>
            "#;
            Write::write_all(&mut zip, core.as_bytes())?;

            // docProps/app.xml
            zip.start_file("docProps/app.xml", options)?;
            let app = r#"
            <Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties">
                <Company>Acme Corp</Company>
            </Properties>
            "#;
            Write::write_all(&mut zip, app.as_bytes())?;

            // docProps/thumbnail.jpeg
            zip.start_file("docProps/thumbnail.jpeg", options)?;
            Write::write_all(&mut zip, b"image data")?;

            zip.finish()?;
        }

        let mut stream = Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;

        assert_eq!(mi.title, "Test Document");
        assert_eq!(mi.authors, vec!["John Doe"]);
        assert_eq!(mi.comments.as_deref(), Some("A test document."));
        assert_eq!(mi.publisher.as_deref(), Some("Acme Corp"));
        assert!(mi.tags.contains(&"tag1".to_string()));
        assert!(mi.cover_data.1.starts_with(b"image data"));

        Ok(())
    }
}

#[cfg(test)]
mod set_metadata_tests {
    use super::*;
    use std::io::Write;

    /// A real DOCX with both property parts, including fields a
    /// regenerating writer would lose.
    fn write_docx(path: &Path, with_app_props: bool) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let deflated = zip::write::FileOptions::default();

        zip.start_file("[Content_Types].xml", deflated).unwrap();
        zip.write_all(br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#).unwrap();

        zip.start_file("word/document.xml", deflated).unwrap();
        zip.write_all(br#"<?xml version="1.0"?><document/>"#).unwrap();

        zip.start_file("docProps/core.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
  <dc:title>Old Title</dc:title>
  <dc:creator>Old Author</dc:creator>
  <dc:language>fr</dc:language>
  <cp:revision>4</cp:revision>
  <dcterms:created xsi:type="dcterms:W3CDTF">2020-01-01T00:00:00Z</dcterms:created>
</cp:coreProperties>"#,
        )
        .unwrap();

        if with_app_props {
            zip.start_file("docProps/app.xml", deflated).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties">
  <Application>Microsoft Office Word</Application>
  <Pages>12</Pages>
</Properties>"#,
            )
            .unwrap();
        }
        zip.finish().unwrap();
    }

    fn a_document(dir: &tempfile::TempDir) -> std::path::PathBuf {
        let path = dir.path().join("doc.docx");
        write_docx(&path, true);
        path
    }

    fn part(path: &Path, name: &str) -> String {
        crate::metadata::zip_edit::read_entry_text(path, name).unwrap()
    }

    #[test]
    fn title_and_authors_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_document(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        mi.authors = vec!["Ann Author".to_string(), "Bob Writer".to_string()];
        set_metadata(&path, &mi).unwrap();

        let read_back = get_metadata(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(read_back.title, "New Title");
        // The reader takes `dc:creator` as one string, so it comes back as
        // the joined form rather than split -- asserted as it really is.
        assert_eq!(read_back.authors, vec!["Ann Author & Bob Writer".to_string()]);
    }

    /// Publisher lives in `app.xml` as `Company`, not in core.xml.
    #[test]
    fn publisher_round_trips_through_app_props() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_document(&dir);

        let mut mi = MetaInformation::default();
        mi.publisher = Some("Real Publisher".to_string());
        set_metadata(&path, &mi).unwrap();

        let read_back = get_metadata(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(read_back.publisher.as_deref(), Some("Real Publisher"));
        // And the rest of app.xml survives.
        let app = part(&path, "docProps/app.xml");
        assert!(app.contains("Microsoft Office Word"), "app.xml lost its Application:\n{app}");
        assert!(app.contains("<Pages>12</Pages>"), "app.xml lost its page count:\n{app}");
    }

    /// `app.xml` is optional. A document without one must still take a
    /// title, and must not fail because a publisher had nowhere to go.
    #[test]
    fn a_document_without_app_props_still_works() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.docx");
        write_docx(&path, false);

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        mi.publisher = Some("Nowhere To Put This".to_string());
        set_metadata(&path, &mi).unwrap();

        let read_back = get_metadata(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(read_back.title, "New Title");
        assert_eq!(read_back.publisher, None, "there is no app.xml to hold a publisher");
    }

    #[test]
    fn tags_are_written_as_comma_joined_keywords() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_document(&dir);

        let mut mi = MetaInformation::default();
        mi.tags = vec!["Science Fiction".to_string(), "Classics".to_string()];
        set_metadata(&path, &mi).unwrap();

        let core = part(&path, "docProps/core.xml");
        assert!(core.contains("Science Fiction, Classics"), "keywords should be comma-joined:\n{core}");

        let read_back = get_metadata(std::fs::File::open(&path).unwrap()).unwrap();
        assert!(read_back.tags.contains(&"Science Fiction".to_string()), "{:?}", read_back.tags);
        assert!(read_back.tags.contains(&"Classics".to_string()), "{:?}", read_back.tags);
    }

    /// Word's own bookkeeping in core.xml is not metadata and must
    /// survive -- including `xsi:type`, a prefixed attribute.
    #[test]
    fn word_bookkeeping_and_prefixed_attributes_survive() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_document(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        set_metadata(&path, &mi).unwrap();

        let core = part(&path, "docProps/core.xml");
        assert!(core.contains("<cp:revision>4</cp:revision>"), "the revision count was lost:\n{core}");
        assert!(core.contains("dcterms:created"), "the creation date was lost:\n{core}");
        assert!(core.contains(r#"xsi:type="dcterms:W3CDTF""#), "a prefixed attribute was lost:\n{core}");
    }

    /// Same bug the ODT tests found: a default `MetaInformation` must not
    /// write calibre's placeholders over real values.
    #[test]
    fn placeholder_metadata_does_not_overwrite_real_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_document(&dir);

        set_metadata(&path, &MetaInformation::default()).unwrap();

        let read_back = get_metadata(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(read_back.title, "Old Title", "the real title was overwritten with a placeholder");
        assert_eq!(read_back.authors, vec!["Old Author".to_string()], "the real author was overwritten");
    }

    #[test]
    fn repeated_edits_do_not_accumulate_elements() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_document(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "First".to_string();
        set_metadata(&path, &mi).unwrap();
        mi.title = "Second".to_string();
        set_metadata(&path, &mi).unwrap();

        let core = part(&path, "docProps/core.xml");
        assert_eq!(core.matches("<dc:title").count(), 1, "titles accumulated:\n{core}");
        assert!(core.contains("Second"));
    }

    #[test]
    fn the_document_body_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_document(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "New".to_string();
        set_metadata(&path, &mi).unwrap();

        assert!(part(&path, "word/document.xml").contains("<document"), "the document body should still be there");
        assert!(part(&path, "[Content_Types].xml").contains("Types"), "the content-type map should still be there");
    }
}
