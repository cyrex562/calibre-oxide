use crate::metadata::MetaInformation;
use crate::opf::parse_opf;
use anyhow::{bail, Context, Result};
use std::io::{Read, Seek};
use zip::ZipArchive;

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    let mut archive = ZipArchive::new(&mut stream).context("Failed to read zip archive")?;

    // Find the first .opf file in the archive
    let mut opf_name = String::new();
    for i in 0..archive.len() {
        let file = archive.by_index(i)?;
        let name = file.name();
        if name.ends_with(".opf") && !name.contains('/') {
            opf_name = name.to_string();
            break;
        }
    }

    if opf_name.is_empty() {
        bail!("No OPF found in archive");
    }

    // Read OPF content
    let opf_content = {
        let mut f = archive.by_name(&opf_name)?;
        let mut s = String::new();
        f.read_to_string(&mut s)?;
        s
    };

    // Parse Metadata
    let mut mi = parse_opf(&opf_content)?;

    // Extract cover if available
    let mut cover_href = None;

    // Check raster cover / guide cover logic (simplified from Python)
    // 1. Check if cover_id is set
    if let Some(cover_id) = &mi.cover_id {
        // Find href for this ID
        // Simplified search in OPF content for href associated with ID
        // Better: parse_opf should probably return this map, but currently returns MetaInformation.
        // We'll use a helper helper similar to epub if needed, or parse_opf ensures it sets something?
        // Actually, parse_opf sets `cover_id`. Use simple XML scan for ID -> Href match if not exposed.
        cover_href = find_href_by_id(&opf_content, cover_id);
    }

    // Python fallback logic: check for meta name="cover", guide items, etc.
    // parse_opf likely handles the 'meta name="cover"' -> sets cover_id.

    // If we have an href, verify it exists and is an image
    if let Some(href) = cover_href {
        if let Ok(mut file) = archive.by_name(&href) {
            let mut data = Vec::new();
            file.read_to_end(&mut data)?;
            let ext = std::path::Path::new(&href)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("jpg")
                .to_string();
            mi.cover_data = (Some(ext), data);
        }
    }

    Ok(mi)
}

fn find_href_by_id(xml: &str, id: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    // <manifest><item id="..." href="..."/></manifest>
    root.descendants()
        .find(|n| n.tag_name().name().eq_ignore_ascii_case("item") && n.attribute("id") == Some(id))
        .and_then(|n| n.attribute("href").map(|s| s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::FileOptions;

    #[test]
    fn test_extz_metadata() -> Result<()> {
        let mut buffer = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buffer));
            let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);

            // metadata.opf
            zip.start_file("metadata.opf", options)?;
            let opf = r#"
            <package xmlns="http://www.idpf.org/2007/opf" version="2.0">
                <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
                    <dc:title>EXTZ Title</dc:title>
                    <dc:creator>EXTZ Author</dc:creator>
                    <meta name="cover" content="cover-id"/>
                </metadata>
                <manifest>
                    <item id="cover-id" href="cover.jpg" media-type="image/jpeg"/>
                </manifest>
            </package>
            "#;
            Write::write_all(&mut zip, opf.as_bytes())?;

            // cover.jpg
            zip.start_file("cover.jpg", options)?;
            Write::write_all(&mut zip, b"extz cover")?;

            zip.finish()?;
        }

        let mut stream = Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;

        assert_eq!(mi.title, "EXTZ Title");
        assert_eq!(mi.authors, vec!["EXTZ Author"]);
        assert!(mi.cover_data.1.starts_with(b"extz cover"));

        Ok(())
    }
}

/// Writes `mi` into an EXTZ archive's OPF, in place (#834).
///
/// EXTZ is the "extension zip" family -- `.htmlz` and `.txtz` -- a zip of
/// content plus an OPF. So this is the same edit
/// [`crate::metadata::epub::set_metadata`] performs, sharing its
/// `rewrite_opf`; only locating the OPF differs, since there is no
/// `META-INF/container.xml` to consult. The first `.opf` in the archive is
/// the one, matching `get_metadata` above and upstream's own
/// `get_first_opf_name`.
///
/// **Disclosed narrowing:** upstream also replaces the cover image, adding
/// a `cover.jpg` and pointing the OPF at it when the book has none. That
/// needs the manifest edited as well as the metadata, and is worth its own
/// change rather than a half-version here.
pub fn set_metadata(path: &std::path::Path, mi: &MetaInformation) -> Result<()> {
    let opf_name = first_opf_name(path)?;
    let opf = crate::metadata::zip_edit::read_entry_text(path, &opf_name)?;
    let updated = crate::metadata::epub::rewrite_opf(&opf, mi)?;
    crate::metadata::zip_edit::replace_entry(path, &opf_name, &updated)
}

/// The first `.opf` entry in the archive.
fn first_opf_name(path: &std::path::Path) -> Result<String> {
    let mut archive = ZipArchive::new(std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?)?;
    for i in 0..archive.len() {
        let name = archive.by_index(i)?.name().to_string();
        if name.to_lowercase().ends_with(".opf") {
            return Ok(name);
        }
    }
    bail!("{} contains no OPF, so it has nowhere to record metadata", path.display())
}

#[cfg(test)]
mod set_metadata_tests {
    use super::*;
    use std::io::Write;

    /// A real HTMLZ: a zip of content plus an OPF, which is what this
    /// project's own HTMLZ output produces.
    fn an_htmlz(dir: &tempfile::TempDir) -> std::path::PathBuf {
        let path = dir.path().join("book.htmlz");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let options = zip::write::FileOptions::default();

        zip.start_file("index.html", options).unwrap();
        zip.write_all(b"<html><body><p>The quick brown fox.</p></body></html>").unwrap();

        zip.start_file("metadata.opf", options).unwrap();
        zip.write_all(
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:title>Old Title</dc:title>
    <dc:creator opf:role="aut">Old Author</dc:creator>
    <dc:language>fr</dc:language>
    <dc:identifier id="uid">urn:uuid:11111111-1111-1111-1111-111111111111</dc:identifier>
  </metadata>
  <manifest><item id="html" href="index.html" media-type="application/xhtml+xml"/></manifest>
  <spine><itemref idref="html"/></spine>
</package>"#,
        )
        .unwrap();
        zip.finish().unwrap();
        path
    }

    /// The read side was missing from the dispatcher entirely, so a HTMLZ --
    /// a format this project both converts to and from -- had no readable
    /// title at all.
    #[test]
    fn the_dispatcher_now_reads_an_htmlz() {
        let dir = tempfile::tempdir().unwrap();
        let path = an_htmlz(&dir);

        let mi = crate::metadata::get_metadata(&path).expect("htmlz should be a readable format");
        assert_eq!(mi.title, "Old Title");
    }

    #[test]
    fn metadata_round_trips_through_the_dispatcher() {
        let dir = tempfile::tempdir().unwrap();
        let path = an_htmlz(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "Corrected Title".to_string();
        mi.authors = vec!["Ann Author".to_string()];
        crate::metadata::set_metadata(&path, &mi).unwrap();

        let read_back = crate::metadata::get_metadata(&path).unwrap();
        assert_eq!(read_back.title, "Corrected Title");
        assert_eq!(read_back.authors, vec!["Ann Author".to_string()]);
    }

    /// Same invariant the EPUB writer has, inherited by sharing its
    /// `rewrite_opf`: writing an ISBN must not strip the book's identity.
    #[test]
    fn the_unique_identifier_survives() {
        let dir = tempfile::tempdir().unwrap();
        let path = an_htmlz(&dir);

        let mut mi = MetaInformation::default();
        mi.identifiers.insert("isbn".to_string(), "9780441013593".to_string());
        set_metadata(&path, &mi).unwrap();

        let opf = crate::metadata::zip_edit::read_entry_text(&path, "metadata.opf").unwrap();
        assert!(opf.contains("urn:uuid:11111111-1111-1111-1111-111111111111"), "the unique identifier was lost:\n{opf}");
        assert!(opf.contains("9780441013593"), "the ISBN was not written:\n{opf}");
    }

    /// And the placeholder rule, likewise inherited.
    #[test]
    fn placeholder_metadata_does_not_overwrite_real_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = an_htmlz(&dir);

        set_metadata(&path, &MetaInformation::default()).unwrap();

        let read_back = crate::metadata::get_metadata(&path).unwrap();
        assert_eq!(read_back.title, "Old Title");
        assert_eq!(read_back.languages, vec!["fr".to_string()]);
    }

    #[test]
    fn the_content_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = an_htmlz(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "New".to_string();
        set_metadata(&path, &mi).unwrap();

        assert!(crate::metadata::zip_edit::read_entry_text(&path, "index.html").unwrap().contains("quick brown fox"));
    }

    /// An archive with no OPF has nowhere to record metadata, and saying so
    /// beats writing one whose manifest describes nothing.
    #[test]
    fn an_archive_without_an_opf_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bare.htmlz");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        zip.start_file("index.html", zip::write::FileOptions::default()).unwrap();
        zip.write_all(b"<html><body><p>No OPF here.</p></body></html>").unwrap();
        zip.finish().unwrap();

        let err = set_metadata(&path, &MetaInformation::default()).unwrap_err();
        assert!(format!("{err:#}").contains("no OPF"), "{err:#}");
    }
}
