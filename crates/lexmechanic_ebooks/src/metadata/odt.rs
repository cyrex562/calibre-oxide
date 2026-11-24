use crate::metadata::{string_to_authors, MetaInformation};
use crate::metadata::zip_edit::placeholders;
use crate::xmltree::{Xml, XmlNodeId};
use anyhow::{Context, Result};
use std::io::{Read, Seek};
use std::path::Path;
use zip::ZipArchive;

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    let mut archive = ZipArchive::new(&mut stream)?;

    // Read meta.xml
    let mut meta_file = archive.by_name("meta.xml").context("No meta.xml in ODT")?;

    let mut xml = String::new();
    meta_file.read_to_string(&mut xml)?;

    parse_metadata(&xml)
}

fn parse_metadata(xml: &str) -> Result<MetaInformation> {
    let doc = roxmltree::Document::parse(xml)?;
    let mut mi = MetaInformation::default();

    // Namespaces in roxmltree are handled via Uri.
    // DC = http://purl.org/dc/elements/1.1/
    // META = urn:oasis:names:tc:opendocument:xmlns:meta:1.0

    const DC_NS: &str = "http://purl.org/dc/elements/1.1/";
    const META_NS: &str = "urn:oasis:names:tc:opendocument:xmlns:meta:1.0";

    // Traverse
    for node in doc.descendants() {
        if node.is_element() {
            if node.tag_name().namespace() == Some(DC_NS) {
                match node.tag_name().name() {
                    "title" => {
                        if let Some(t) = node.text() {
                            mi.title = t.trim().to_string();
                        }
                    }
                    "creator" | "initial-creator" => {
                        if let Some(t) = node.text() {
                            mi.authors = string_to_authors(t);
                        }
                    }
                    "description" => {
                        if let Some(t) = node.text() {
                            mi.comments = Some(t.trim().to_string());
                        }
                    }
                    "language" => {
                        if let Some(t) = node.text() {
                            mi.languages = vec![t.trim().to_string()];
                        }
                    }
                    "subject" => {
                        if let Some(t) = node.text() {
                            mi.tags.push(t.trim().to_string());
                        }
                    }
                    _ => {}
                }
            } else if node.tag_name().namespace() == Some(META_NS) {
                match node.tag_name().name() {
                    "keyword" => {
                        if let Some(t) = node.text() {
                            mi.tags.push(t.trim().to_string());
                        }
                    }
                    "user-defined" => {
                        // Custom metadata
                        if let Some(name) = node.attribute((META_NS, "name")) {
                            if let Some(val) = node.text() {
                                mi.user_metadata
                                    .insert(name.to_lowercase(), val.to_string());

                                // Map back to Calibre fields if possible
                                match name.to_lowercase().as_str() {
                                    "opf.series" => mi.series = Some(val.to_string()),
                                    "opf.seriesindex" => {
                                        if let Ok(idx) = val.parse::<f64>() {
                                            mi.series_index = idx;
                                        }
                                    }
                                    "opf.publisher" => mi.publisher = Some(val.to_string()),
                                    _ => {}
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(mi)
}

const DC_NS: &str = "http://purl.org/dc/elements/1.1/";
const META_NS: &str = "urn:oasis:names:tc:opendocument:xmlns:meta:1.0";
const OFFICE_NS: &str = "urn:oasis:names:tc:opendocument:xmlns:office:1.0";

/// Writes `mi` into an ODT's `meta.xml`, in place (#834).
///
/// The same edit-don't-regenerate rule as
/// [`crate::metadata::epub::set_metadata`]: `office:meta` also holds
/// generator strings, editing-cycle counts and statistics that belong to
/// the document rather than to its metadata, and regenerating the element
/// would throw them away.
///
/// ODT has no `dc:publisher`, so a publisher round-trips through
/// `meta:user-defined name="opf.publisher"` -- which is exactly where
/// [`get_metadata`] reads it from, and what upstream's own ODT writer
/// uses. Series goes the same way.
pub fn set_metadata(path: &Path, mi: &MetaInformation) -> Result<()> {
    let meta_xml = crate::metadata::zip_edit::read_entry_text(path, "meta.xml")?;
    let updated = rewrite_meta(&meta_xml, mi)?;
    crate::metadata::zip_edit::replace_entry(path, "meta.xml", &updated)
}

/// Replaces every `<ns:local>` child of `parent` with one element per value.
///
/// An empty `values` means "leave this field alone", not "clear it" -- the
/// caller is setting fields, not declaring the complete set.
fn replace_children(xml: &mut Xml, parent: XmlNodeId, ns: &str, local: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    for child in xml.element_children(parent) {
        if xml.namespace(child) == Some(ns) && xml.local_name(child) == Some(local) {
            xml.detach(child);
        }
    }
    for value in values {
        let element = xml.new_element(local, Some(ns));
        xml.set_element_text(element, value.as_str());
        xml.insert_element(parent, element, None);
    }
}

/// Sets a `meta:user-defined` entry, which is how ODT carries the fields
/// its own vocabulary has no element for.
fn set_user_defined(xml: &mut Xml, parent: XmlNodeId, name: &str, value: &str) {
    for child in xml.element_children(parent) {
        if xml.namespace(child) != Some(META_NS) || xml.local_name(child) != Some("user-defined") {
            continue;
        }
        if xml.get_attr(child, "meta:name") == Some(name) {
            xml.set_element_text(child, value);
            return;
        }
    }
    let element = xml.new_element("user-defined", Some(META_NS));
    xml.set_attr(element, "meta:name", name);
    xml.set_element_text(element, value);
    xml.insert_element(parent, element, None);
}

fn rewrite_meta(meta_xml: &str, mi: &MetaInformation) -> Result<Vec<u8>> {
    let mut xml = Xml::parse(meta_xml).context("parsing meta.xml")?;
    let root = xml.root_element().context("meta.xml has no root element")?;

    // `office:meta` is the container. A `meta.xml` without one is not
    // something to guess at.
    let meta = xml
        .element_children(root)
        .into_iter()
        .find(|&c| xml.namespace(c) == Some(OFFICE_NS) && xml.local_name(c) == Some("meta"))
        .context("meta.xml has no office:meta element")?;

    xml.ensure_namespace_declared(Some("dc"), DC_NS);
    xml.ensure_namespace_declared(Some("meta"), META_NS);

    if let Some(title) = placeholders::real_title(&mi.title) {
        replace_children(&mut xml, meta, DC_NS, "title", &[title.to_string()]);
    }
    if let Some(authors) = placeholders::real_authors(&mi.authors) {
        // ODT's `dc:creator` is a single string, not one element per
        // author -- so they are joined the way `string_to_authors` reads
        // them back, rather than written as repeated elements the reader
        // would collapse to the last one.
        replace_children(&mut xml, meta, DC_NS, "creator", &[authors.join(" & ")]);
    }
    if let Some(comments) = mi.comments.as_ref().filter(|c| !c.trim().is_empty()) {
        replace_children(&mut xml, meta, DC_NS, "description", std::slice::from_ref(comments));
    }
    if let Some(languages) = placeholders::real_languages(&mi.languages) {
        replace_children(&mut xml, meta, DC_NS, "language", &languages[..1]);
    }
    // Tags become `meta:keyword` elements, which is the repeatable one.
    // `dc:subject` is single-valued, so using it would silently drop all
    // but one tag.
    replace_children(&mut xml, meta, META_NS, "keyword", &mi.tags);

    if let Some(publisher) = mi.publisher.as_ref().filter(|p| !p.trim().is_empty()) {
        set_user_defined(&mut xml, meta, "opf.publisher", publisher);
    }
    if let Some(series) = mi.series.as_ref().filter(|s| !s.trim().is_empty()) {
        set_user_defined(&mut xml, meta, "opf.series", series);
        set_user_defined(&mut xml, meta, "opf.seriesindex", &mi.series_index.to_string());
    }

    Ok(xml.serialize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::FileOptions;

    #[test]
    fn test_odt_metadata() -> Result<()> {
        let mut buffer = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buffer));
            let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zip.start_file("meta.xml", options)?;

            let xml = r#"
            <office:document-meta xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
                                  xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0"
                                  xmlns:dc="http://purl.org/dc/elements/1.1/">
                <office:meta>
                    <dc:title>My ODT Title</dc:title>
                    <dc:creator>Jane Doe</dc:creator>
                    <dc:description>Description</dc:description>
                    <meta:keyword>tag1</meta:keyword>
                    <meta:user-defined meta:name="opf.series">My Series</meta:user-defined>
                    <meta:user-defined meta:name="opf.seriesindex">2.5</meta:user-defined>
                </office:meta>
            </office:document-meta>
            "#;
            zip.write_all(xml.as_bytes())?;
            zip.finish()?;
        }

        let mut stream = Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;

        assert_eq!(mi.title, "My ODT Title");
        assert_eq!(mi.authors, vec!["Jane Doe"]);
        assert_eq!(mi.comments, Some("Description".to_string()));
        assert_eq!(mi.tags, vec!["tag1"]);
        assert_eq!(mi.series, Some("My Series".to_string()));
        assert_eq!(mi.series_index, 2.5);

        Ok(())
    }
}

#[cfg(test)]
mod set_metadata_tests {
    use super::*;
    use std::io::Write;

    /// A real ODT whose `meta.xml` carries document-level things a
    /// regenerating writer would destroy.
    fn write_odt(path: &Path) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let deflated = zip::write::FileOptions::default();

        zip.start_file("mimetype", zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap();
        zip.write_all(b"application/vnd.oasis.opendocument.text").unwrap();

        zip.start_file("meta.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-meta xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0" office:version="1.2">
  <office:meta>
    <meta:generator>LibreOffice/7.4</meta:generator>
    <dc:title>Old Title</dc:title>
    <dc:creator>Old Author</dc:creator>
    <dc:language>fr</dc:language>
    <meta:editing-cycles>7</meta:editing-cycles>
    <meta:document-statistic meta:page-count="12" meta:word-count="3400"/>
  </office:meta>
</office:document-meta>"#,
        )
        .unwrap();

        zip.start_file("content.xml", deflated).unwrap();
        zip.write_all(br#"<?xml version="1.0"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"/>"#).unwrap();
        zip.finish().unwrap();
    }

    fn a_document(dir: &tempfile::TempDir) -> std::path::PathBuf {
        let path = dir.path().join("doc.odt");
        write_odt(&path);
        path
    }

    fn meta_of(path: &Path) -> String {
        crate::metadata::zip_edit::read_entry_text(path, "meta.xml").unwrap()
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
        assert_eq!(read_back.authors, vec!["Ann Author".to_string(), "Bob Writer".to_string()]);
    }

    /// ODT's `dc:creator` is single-valued. Writing one element per author
    /// would have the reader keep only the last, silently losing the rest.
    #[test]
    fn multiple_authors_are_joined_into_one_creator_element() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_document(&dir);

        let mut mi = MetaInformation::default();
        mi.authors = vec!["Ann Author".to_string(), "Bob Writer".to_string()];
        set_metadata(&path, &mi).unwrap();

        let meta = meta_of(&path);
        assert_eq!(meta.matches("<dc:creator").count(), 1, "there should be exactly one creator element:\n{meta}");
    }

    /// Tags go to repeatable `meta:keyword`, not single-valued
    /// `dc:subject`, or all but one would be lost.
    #[test]
    fn tags_become_repeated_keyword_elements() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_document(&dir);

        let mut mi = MetaInformation::default();
        mi.tags = vec!["Science Fiction".to_string(), "Classics".to_string()];
        set_metadata(&path, &mi).unwrap();

        let read_back = get_metadata(std::fs::File::open(&path).unwrap()).unwrap();
        assert!(read_back.tags.contains(&"Science Fiction".to_string()), "{:?}", read_back.tags);
        assert!(read_back.tags.contains(&"Classics".to_string()), "{:?}", read_back.tags);
    }

    /// ODT has no `dc:publisher`; it round-trips through the
    /// `opf.publisher` user-defined field the reader already looks for.
    #[test]
    fn publisher_and_series_round_trip_through_user_defined_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_document(&dir);

        let mut mi = MetaInformation::default();
        mi.publisher = Some("Real Publisher".to_string());
        mi.series = Some("A Series".to_string());
        mi.series_index = 3.0;
        set_metadata(&path, &mi).unwrap();

        let read_back = get_metadata(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(read_back.publisher.as_deref(), Some("Real Publisher"));
        assert_eq!(read_back.series.as_deref(), Some("A Series"));
        assert_eq!(read_back.series_index, 3.0);
    }

    /// The document's own bookkeeping is not metadata and must survive.
    #[test]
    fn generator_and_statistics_survive() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_document(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        set_metadata(&path, &mi).unwrap();

        let meta = meta_of(&path);
        assert!(meta.contains("LibreOffice/7.4"), "the generator was lost:\n{meta}");
        assert!(meta.contains("editing-cycles"), "the editing-cycle count was lost:\n{meta}");
        assert!(meta.contains("document-statistic"), "the statistics were lost:\n{meta}");
        assert!(meta.contains(r#"meta:page-count="12""#), "a prefixed attribute was lost:\n{meta}");
    }

    #[test]
    fn a_field_the_caller_did_not_set_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_document(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        set_metadata(&path, &mi).unwrap();

        let read_back = get_metadata(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(read_back.languages, vec!["fr".to_string()], "the language should have been left as it was");
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

        let meta = meta_of(&path);
        assert_eq!(meta.matches("<dc:title").count(), 1, "titles accumulated:\n{meta}");
        assert!(meta.contains("Second"));
    }

    #[test]
    fn the_content_and_mimetype_are_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_document(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "New".to_string();
        set_metadata(&path, &mi).unwrap();

        let mut archive = ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let first = archive.by_index(0).unwrap();
        assert_eq!(first.name(), "mimetype");
        assert_eq!(first.compression(), zip::CompressionMethod::Stored);
        drop(first);
        assert!(archive.by_name("content.xml").is_ok(), "the content should still be there");
    }
}
