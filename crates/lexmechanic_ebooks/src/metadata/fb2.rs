use crate::metadata::zip_edit::placeholders;
use crate::metadata::MetaInformation;
use crate::xmltree::{Xml, XmlNodeId};
use anyhow::{Context, Result};
use base64::{engine::general_purpose, Engine as _};
use roxmltree::Document;
use std::io::{Read, Seek};
use std::path::Path;
use zip::ZipArchive;

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    // Check if zip
    let start_pos = stream.stream_position()?;
    let mut xml_content = String::new();
    let mut is_zip = false;

    if let Ok(mut archive) = ZipArchive::new(&mut stream) {
        // Find .fb2 file
        let mut file_name = String::new();
        for i in 0..archive.len() {
            let file = archive.by_index(i)?;
            if file.name().to_lowercase().ends_with(".fb2") {
                file_name = file.name().to_string();
                break;
            }
        }
        if !file_name.is_empty() {
            let mut file = archive.by_name(&file_name)?;
            file.read_to_string(&mut xml_content)?;
            is_zip = true;
        }
    }

    if !is_zip {
        stream.seek(std::io::SeekFrom::Start(start_pos))?;
        stream.read_to_string(&mut xml_content)?;
    }

    parse_fb2(&xml_content)
}

const FB2_NS: &str = "http://www.gribuser.ru/xml/fictionbook/2.0";

/// Writes `mi` into an FB2 file, in place (#834).
///
/// Port of `metadata/fb2.py`'s `set_metadata`. Handles both shapes the
/// reader accepts: a bare `.fb2` XML file, and a zipped one, where the
/// `.fb2` entry inside is rewritten and the rest of the archive left
/// alone.
///
/// **Disclosed narrowing:** the cover (`<coverpage>` plus a `<binary>`
/// element holding base64 image data) is not written. Upstream's
/// `_set_cover` adds both, and doing it properly means managing the
/// binary payloads the rest of the document references by id -- worth its
/// own change rather than a half-version here.
pub fn set_metadata(path: &Path, mi: &MetaInformation) -> Result<()> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;

    // A zipped FB2 keeps its XML in an inner entry; find it the same way
    // `get_metadata` does rather than guessing from the filename, since
    // `.fb2` and `.fb2.zip` are both spelled `.fb2` by some producers.
    let inner = zipped_fb2_entry(&bytes);
    let xml_text = match &inner {
        Some(name) => crate::metadata::zip_edit::read_entry_text(path, name)?,
        None => String::from_utf8_lossy(&bytes).into_owned(),
    };

    let updated = rewrite_fb2(&xml_text, mi)?;

    match &inner {
        Some(name) => crate::metadata::zip_edit::replace_entry(path, name, &updated),
        None => {
            // Written beside the original and renamed, so an interrupted
            // write cannot truncate the book.
            let staging = tempfile::Builder::new().prefix("set-metadata").suffix(".fb2").tempfile_in(path.parent().unwrap_or(Path::new(".")))?;
            std::fs::write(staging.path(), &updated)?;
            staging.persist(path).map_err(|e| anyhow::anyhow!("replacing {}: {e}", path.display()))?;
            Ok(())
        }
    }
}

/// The name of the `.fb2` entry inside a zipped FB2, or `None` for a bare
/// XML file.
fn zipped_fb2_entry(bytes: &[u8]) -> Option<String> {
    let mut archive = ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    for i in 0..archive.len() {
        let name = archive.by_index(i).ok()?.name().to_string();
        if name.to_lowercase().ends_with(".fb2") {
            return Some(name);
        }
    }
    None
}

/// Replaces every `local` child of `parent` with one element per value.
fn replace_all(xml: &mut Xml, parent: XmlNodeId, local: &str, values: &[String]) {
    for child in xml.element_children(parent) {
        if xml.local_name(child) == Some(local) {
            xml.detach(child);
        }
    }
    for value in values {
        let element = xml.new_element(local, Some(FB2_NS));
        xml.set_element_text(element, value.as_str());
        xml.insert_element(parent, element, None);
    }
}

/// Finds a child by local name, creating it if absent -- upstream's
/// `Context.get_or_create`.
fn get_or_create(xml: &mut Xml, parent: XmlNodeId, local: &str) -> XmlNodeId {
    if let Some(found) = xml.element_children(parent).into_iter().find(|&c| xml.local_name(c) == Some(local)) {
        return found;
    }
    let element = xml.new_element(local, Some(FB2_NS));
    xml.insert_element(parent, element, None);
    element
}

/// Splits a display name into FB2's structured name elements.
///
/// Exactly upstream's `_set_authors` rule: one word becomes a
/// `<nickname>`, two become first/last, and three or more put everything
/// after the second into `<last-name>`. Guessy, but it is the guess every
/// other FB2 tool makes, and matching it is what makes names survive a
/// round trip through them.
fn author_name_parts(author: &str) -> Vec<(&'static str, String)> {
    let parts: Vec<&str> = author.split_whitespace().collect();
    match parts.len() {
        0 => Vec::new(),
        1 => vec![("nickname", parts[0].to_string())],
        2 => vec![("first-name", parts[0].to_string()), ("last-name", parts[1].to_string())],
        _ => vec![
            ("first-name", parts[0].to_string()),
            ("middle-name", parts[1].to_string()),
            ("last-name", parts[2..].join(" ")),
        ],
    }
}

fn rewrite_fb2(xml_text: &str, mi: &MetaInformation) -> Result<Vec<u8>> {
    let mut xml = Xml::parse(xml_text).context("parsing the FB2 XML")?;
    let root = xml.root_element().context("the FB2 file has no root element")?;

    let description = get_or_create(&mut xml, root, "description");
    let title_info = get_or_create(&mut xml, description, "title-info");

    if let Some(title) = placeholders::real_title(&mi.title) {
        replace_all(&mut xml, title_info, "book-title", &[title.to_string()]);
    }
    if let Some(authors) = placeholders::real_authors(&mi.authors) {
        for child in xml.element_children(title_info) {
            if xml.local_name(child) == Some("author") {
                xml.detach(child);
            }
        }
        for author in authors {
            let parts = author_name_parts(author);
            if parts.is_empty() {
                continue;
            }
            let element = xml.new_element("author", Some(FB2_NS));
            for (local, text) in parts {
                let part = xml.new_element(local, Some(FB2_NS));
                xml.set_element_text(part, text.as_str());
                xml.insert_element(element, part, None);
            }
            xml.insert_element(title_info, element, None);
        }
    }
    if !mi.tags.is_empty() {
        replace_all(&mut xml, title_info, "genre", &mi.tags);
    }
    if let Some(languages) = placeholders::real_languages(&mi.languages) {
        replace_all(&mut xml, title_info, "lang", &languages[..1]);
    }
    if let Some(comments) = mi.comments.as_ref().filter(|c| !c.trim().is_empty()) {
        // An annotation's content is block markup, so the text goes in a
        // `<p>` rather than directly in `<annotation>` -- an FB2 reader
        // expects paragraphs there.
        for child in xml.element_children(title_info) {
            if xml.local_name(child) == Some("annotation") {
                xml.detach(child);
            }
        }
        let annotation = xml.new_element("annotation", Some(FB2_NS));
        let paragraph = xml.new_element("p", Some(FB2_NS));
        xml.set_element_text(paragraph, comments.as_str());
        xml.insert_element(annotation, paragraph, None);
        xml.insert_element(title_info, annotation, None);
    }
    if let Some(series) = mi.series.as_ref().filter(|s| !s.trim().is_empty()) {
        for child in xml.element_children(title_info) {
            if xml.local_name(child) == Some("sequence") {
                xml.detach(child);
            }
        }
        let sequence = xml.new_element("sequence", Some(FB2_NS));
        xml.set_attr(sequence, "name", series.as_str());
        xml.set_attr(sequence, "number", mi.series_index.to_string());
        xml.insert_element(title_info, sequence, None);
    }
    if let Some(publisher) = mi.publisher.as_ref().filter(|p| !p.trim().is_empty()) {
        // Publisher lives in `publish-info`, not `title-info`.
        let publish_info = get_or_create(&mut xml, description, "publish-info");
        replace_all(&mut xml, publish_info, "publisher", &[publisher.to_string()]);
    }

    Ok(xml.serialize())
}

fn parse_fb2(xml: &str) -> Result<MetaInformation> {
    let doc = Document::parse(xml)?;
    let root = doc.root_element();
    // FB2 namespace usually http://www.gribuser.ru/xml/fictionbook/2.0
    // But we iterate descendants so we can just check name

    let description = root
        .descendants()
        .find(|n| n.tag_name().name().eq_ignore_ascii_case("description"))
        .ok_or_else(|| anyhow::anyhow!("No description in FB2"))?;

    let title_info = description
        .descendants()
        .find(|n| n.tag_name().name().eq_ignore_ascii_case("title-info"));

    let mut mi = MetaInformation::default();

    if let Some(ti) = title_info {
        // Title
        if let Some(bt) = ti
            .descendants()
            .find(|n| n.tag_name().name().eq_ignore_ascii_case("book-title"))
        {
            if let Some(t) = bt.text() {
                mi.title = t.trim().to_string();
            }
        }
        // Authors
        let authors: Vec<String> = ti
            .descendants()
            .filter(|n| n.tag_name().name().eq_ignore_ascii_case("author"))
            .map(|n| {
                // Compose author name from first-name, middle-name, last-name
                let first = n
                    .children()
                    .find(|c| c.tag_name().name() == "first-name")
                    .and_then(|c| c.text())
                    .unwrap_or("");
                let middle = n
                    .children()
                    .find(|c| c.tag_name().name() == "middle-name")
                    .and_then(|c| c.text())
                    .unwrap_or("");
                let last = n
                    .children()
                    .find(|c| c.tag_name().name() == "last-name")
                    .and_then(|c| c.text())
                    .unwrap_or("");
                // A single-word author is written as `<nickname>` by
                // upstream (and by this port's writer). Composing only
                // first/middle/last dropped those authors entirely.
                let nickname = n
                    .children()
                    .find(|c| c.tag_name().name() == "nickname")
                    .and_then(|c| c.text())
                    .unwrap_or("");

                let full = format!("{} {} {} {}", first, middle, last, nickname);
                full.split_whitespace().collect::<Vec<_>>().join(" ") // normalize spaces
            })
            .filter(|s| !s.is_empty())
            .collect();

        if !authors.is_empty() {
            mi.authors = authors;
        }

        // Series
        if let Some(seq) = ti
            .descendants()
            .find(|n| n.tag_name().name().eq_ignore_ascii_case("sequence"))
        {
            if let Some(name) = seq.attribute("name") {
                mi.series = Some(name.to_string());
            }
            if let Some(num) = seq.attribute("number") {
                if let Ok(idx) = num.parse::<f64>() {
                    mi.series_index = idx;
                }
            }
        }

        // Tags/Genres
        for genre in ti
            .descendants()
            .filter(|n| n.tag_name().name().eq_ignore_ascii_case("genre"))
        {
            if let Some(t) = genre.text() {
                mi.tags.push(t.trim().to_string());
            }
        }

        // Comment/Annotation
        if let Some(annot) = ti
            .descendants()
            .find(|n| n.tag_name().name().eq_ignore_ascii_case("annotation"))
        {
            // Collected from descendants, not `annot.text()`. An
            // annotation's content is block markup -- every real FB2 wraps
            // it in `<p>` -- and `text()` returns only an element's
            // *direct* text, which for `<annotation><p>..</p></annotation>`
            // is nothing. So this read no annotation at all from any real
            // file.
            let mut paragraphs: Vec<String> = Vec::new();
            for node in annot.descendants().filter(|n| n.is_text()) {
                let text = node.text().unwrap_or("").trim();
                if !text.is_empty() {
                    paragraphs.push(text.to_string());
                }
            }
            if !paragraphs.is_empty() {
                mi.comments = Some(paragraphs.join("\n\n"));
            }
        }

        // Language. Was not read at all, so an FB2's `<lang>` never
        // reached the library and every book came back as "und".
        if let Some(lang) = ti
            .children()
            .find(|n| n.tag_name().name().eq_ignore_ascii_case("lang"))
            .and_then(|n| n.text())
        {
            let lang = lang.trim();
            if !lang.is_empty() {
                mi.languages = vec![lang.to_string()];
            }
        }

        // Cover
        // <coverpage><image l:href="#cover.jpg"/></coverpage>
        if let Some(cp) = ti
            .descendants()
            .find(|n| n.tag_name().name().eq_ignore_ascii_case("coverpage"))
        {
            if let Some(img) = cp
                .descendants()
                .find(|n| n.tag_name().name().eq_ignore_ascii_case("image"))
            {
                // href attribute. Might be 'href' or 'l:href' or 'xlink:href'
                // roxmltree handles namespaces.
                // We check all attrs.
                let href = img
                    .attributes()
                    .find(|a| a.name().contains("href"))
                    .map(|a| a.value());
                if let Some(h) = href {
                    let id = h.trim_start_matches('#');
                    // Find <binary id="...">
                    let binary = root
                        .descendants()
                        .find(|n| n.tag_name().name() == "binary" && n.attribute("id") == Some(id));

                    if let Some(bin) = binary {
                        if let Some(content) = bin.text() {
                            // Decode Base64
                            let content = content.split_whitespace().collect::<String>();
                            if let Ok(data) = general_purpose::STANDARD.decode(content) {
                                mi.cover_data = (Some("jpg".to_string()), data);
                                // Assume jpg or check content-type
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(mi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_fb2_metadata() {
        let xml = r##"
        <FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0" xmlns:l="http://www.w3.org/1999/xlink">
        <description>
            <title-info>
                <genre>sf</genre>
                <author>
                    <first-name>John</first-name>
                    <last-name>Doe</last-name>
                </author>
                <book-title>The Book</book-title>
                <sequence name="The Series" number="1"/>
                <coverpage>
                     <image l:href="#cover.jpg"/>
                </coverpage>
            </title-info>
        </description>
        <binary id="cover.jpg" content-type="image/jpeg">
            SGVsbG8=
        </binary>
        </FictionBook>
        "##;

        let mut stream = Cursor::new(xml);
        let mi = get_metadata(&mut stream).unwrap();

        assert_eq!(mi.title, "The Book");
        assert_eq!(mi.authors, vec!["John Doe"]);
        assert_eq!(mi.tags, vec!["sf"]);
        assert_eq!(mi.series, Some("The Series".to_string()));
        assert_eq!(mi.series_index, 1.0);

        // "SGVsbG8=" -> "Hello"
        assert!(mi.cover_data.1.starts_with(b"Hello"));
    }
}

#[cfg(test)]
mod set_metadata_tests {
    use super::*;
    use std::io::Write;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0" xmlns:l="http://www.w3.org/1999/xlink">
  <description>
    <title-info>
      <book-title>Old Title</book-title>
      <author><first-name>Old</first-name><last-name>Author</last-name></author>
      <lang>fr</lang>
    </title-info>
    <document-info>
      <program-used>SomeTool 1.0</program-used>
    </document-info>
  </description>
  <body><section><p>Text</p></section></body>
</FictionBook>"#;

    fn a_bare_fb2(dir: &tempfile::TempDir) -> std::path::PathBuf {
        let path = dir.path().join("book.fb2");
        std::fs::write(&path, SAMPLE).unwrap();
        path
    }

    fn a_zipped_fb2(dir: &tempfile::TempDir) -> std::path::PathBuf {
        let path = dir.path().join("book.fb2.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        zip.start_file("book.fb2", zip::write::FileOptions::default()).unwrap();
        zip.write_all(SAMPLE.as_bytes()).unwrap();
        zip.start_file("readme.txt", zip::write::FileOptions::default()).unwrap();
        zip.write_all(b"not the book").unwrap();
        zip.finish().unwrap();
        path
    }

    fn read_back(path: &Path) -> MetaInformation {
        get_metadata(std::fs::File::open(path).unwrap()).unwrap()
    }

    #[test]
    fn title_and_a_two_part_author_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_bare_fb2(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        mi.authors = vec!["Ann Author".to_string()];
        set_metadata(&path, &mi).unwrap();

        let got = read_back(&path);
        assert_eq!(got.title, "New Title");
        assert_eq!(got.authors, vec!["Ann Author".to_string()]);
    }

    /// A three-part name puts everything after the second word into
    /// `last-name`, as upstream does -- so it must come back whole.
    #[test]
    fn a_three_part_author_name_survives() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_bare_fb2(&dir);

        let mut mi = MetaInformation::default();
        mi.authors = vec!["Ursula K Le Guin".to_string()];
        set_metadata(&path, &mi).unwrap();

        assert_eq!(read_back(&path).authors, vec!["Ursula K Le Guin".to_string()]);
    }

    /// A one-word author becomes a `<nickname>`. The reader used to
    /// compose only first/middle/last, so these vanished entirely.
    #[test]
    fn a_single_word_author_survives_as_a_nickname() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_bare_fb2(&dir);

        let mut mi = MetaInformation::default();
        mi.authors = vec!["Voltaire".to_string()];
        set_metadata(&path, &mi).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("nickname"), "a single-word author should be a nickname:\n{text}");
        assert_eq!(read_back(&path).authors, vec!["Voltaire".to_string()]);
    }

    #[test]
    fn multiple_authors_each_get_their_own_element() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_bare_fb2(&dir);

        let mut mi = MetaInformation::default();
        mi.authors = vec!["Ann Author".to_string(), "Bob Writer".to_string()];
        set_metadata(&path, &mi).unwrap();

        let got = read_back(&path);
        assert_eq!(got.authors, vec!["Ann Author".to_string(), "Bob Writer".to_string()]);
    }

    #[test]
    fn tags_series_and_comments_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_bare_fb2(&dir);

        let mut mi = MetaInformation::default();
        mi.tags = vec!["sf".to_string(), "classics".to_string()];
        mi.series = Some("A Series".to_string());
        mi.series_index = 2.0;
        mi.comments = Some("A short description.".to_string());
        set_metadata(&path, &mi).unwrap();

        let got = read_back(&path);
        assert_eq!(got.tags, vec!["sf".to_string(), "classics".to_string()]);
        assert_eq!(got.series.as_deref(), Some("A Series"));
        assert_eq!(got.series_index, 2.0);
        assert_eq!(got.comments.as_deref(), Some("A short description."));
    }

    /// The zipped shape rewrites the inner `.fb2` and leaves the rest of
    /// the archive alone.
    #[test]
    fn a_zipped_fb2_is_rewritten_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_zipped_fb2(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        set_metadata(&path, &mi).unwrap();

        assert_eq!(read_back(&path).title, "New Title");
        assert_eq!(crate::metadata::zip_edit::read_entry_text(&path, "readme.txt").unwrap(), "not the book", "the other entry should be untouched");
    }

    /// Parts of the document that are not metadata must survive.
    #[test]
    fn the_body_and_document_info_survive() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_bare_fb2(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "New Title".to_string();
        set_metadata(&path, &mi).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("SomeTool 1.0"), "document-info was lost:\n{text}");
        assert!(text.contains("<body>"), "the body was lost:\n{text}");
    }

    #[test]
    fn placeholder_metadata_does_not_overwrite_real_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_bare_fb2(&dir);

        set_metadata(&path, &MetaInformation::default()).unwrap();

        let got = read_back(&path);
        assert_eq!(got.title, "Old Title");
        assert_eq!(got.authors, vec!["Old Author".to_string()]);
        assert_eq!(got.languages, vec!["fr".to_string()]);
    }

    #[test]
    fn repeated_edits_do_not_accumulate_elements() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_bare_fb2(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "First".to_string();
        set_metadata(&path, &mi).unwrap();
        mi.title = "Second".to_string();
        set_metadata(&path, &mi).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("<book-title").count(), 1, "titles accumulated:\n{text}");
    }
}
