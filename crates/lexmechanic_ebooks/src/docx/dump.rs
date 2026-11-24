//! Explode a DOCX to a directory with its XML pretty-printed.
//!
//! Port of `old_src/src/calibre/ebooks/docx/dump.py` — a debugging aid
//! that unzips a package and re-indents every `.xml` and `.rels` part,
//! since Word writes them as one enormous line.
//!
//! The Python leans on `lxml`'s `pretty_print`; this port carries its
//! own serializer, because the parser used everywhere else in this
//! module (`roxmltree`) is read-only. Namespace declarations are
//! hoisted onto the root element, which is where OOXML producers put
//! them anyway.

use std::path::{Path, PathBuf};

use roxmltree::Document;

use super::container::Docx;
use super::error::DocxError;

/// Extract `path` into `dest`, replacing `dest` if it exists, and
/// pretty-print the XML parts.
///
/// Port of the Python `do_dump`.
pub fn do_dump(path: impl AsRef<Path>, dest: impl AsRef<Path>) -> Result<Vec<String>, DocxError> {
    let dest = dest.as_ref();
    if dest.exists() {
        std::fs::remove_dir_all(dest)?;
    }
    let mut docx = Docx::open(path)?;
    let written = docx.extract_to(dest)?;
    pretty_all_xml_in_dir(dest)?;
    Ok(written)
}

/// The directory the Python `dump` would write to for a given input:
/// the file's stem plus `-dumped`, in the current directory.
///
/// Port of the Python `dump`'s destination naming.
pub fn default_dest(path: impl AsRef<Path>) -> PathBuf {
    let stem = path
        .as_ref()
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "docx".to_string());
    PathBuf::from(format!("{stem}-dumped"))
}

/// Re-indent every `.xml` and `.rels` file under `path`, in place.
///
/// Port of the Python `pretty_all_xml_in_dir`. A file that does not
/// parse is left exactly as it was — this is a debugging tool, and
/// destroying the evidence would defeat the purpose.
pub fn pretty_all_xml_in_dir(path: impl AsRef<Path>) -> Result<Vec<PathBuf>, DocxError> {
    let mut done = Vec::new();
    let mut stack = vec![path.as_ref().to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            // Matched on the whole name, not `Path::extension`: the
            // package relationship parts are called `.rels`, which
            // `extension()` reports as a stem with no extension.
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            let is_xml = name.ends_with(".xml") || name.ends_with(".rels");
            if !is_xml {
                continue;
            }
            let raw = std::fs::read(&p)?;
            if raw.is_empty() {
                continue;
            }
            let text = String::from_utf8_lossy(&raw).into_owned();
            let Ok(doc) = Document::parse(&text) else {
                continue;
            };
            std::fs::write(&p, pretty_print(&doc))?;
            done.push(p);
        }
    }
    done.sort();
    Ok(done)
}

/// Serialize a parsed document with one element per line.
///
/// Promoted to [`crate::xml_util::pretty_print`] (issue #145) so other
/// formats can share it; re-exported here under its original name.
pub use crate::xml_util::pretty_print;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    const SRC: &str = concat!(
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#,
        r#"<w:body><w:p w:rsidR="00A1"><w:r><w:t xml:space="preserve">a &amp; b </w:t></w:r></w:p>"#,
        r#"<w:sectPr/></w:body></w:document>"#
    );

    #[test]
    fn pretty_printing_indents_and_round_trips() {
        let doc = Document::parse(SRC).unwrap();
        let out = pretty_print(&doc);
        assert!(out.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n"));
        assert!(out.contains("\n  <w:body>\n"), "indented:\n{out}");
        assert!(out.contains("<w:sectPr/>"), "empty elements self-close");

        // The point of the exercise: the result must still parse, and
        // to the same content.
        let round = Document::parse(&out).expect("pretty output reparses");
        let text: String = round
            .descendants()
            .filter(|n| n.is_text())
            .filter_map(|n| n.text())
            .filter(|t| !t.trim().is_empty())
            .collect();
        assert_eq!(text, "a & b ");
    }

    #[test]
    fn attributes_keep_their_prefixes_and_escaping() {
        let doc = Document::parse(SRC).unwrap();
        let out = pretty_print(&doc);
        assert!(out.contains(r#"w:rsidR="00A1""#), "got:\n{out}");
        assert!(out.contains(r#"xml:space="preserve""#), "got:\n{out}");
    }

    #[test]
    fn text_with_markup_characters_is_escaped() {
        let doc = Document::parse(r#"<a x="&quot;q&quot;">1 &lt; 2 &amp; 3</a>"#).unwrap();
        let out = pretty_print(&doc);
        assert!(out.contains("1 &lt; 2 &amp; 3"), "got: {out}");
        assert!(out.contains(r#"x="&quot;q&quot;""#), "got: {out}");
        Document::parse(&out).expect("reparses");
    }

    fn sample_docx() -> Vec<u8> {
        const CONTENT_TYPES: &str = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/></Types>"#;
        const RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
            let options = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, content) in [
                ("[Content_Types].xml", CONTENT_TYPES),
                ("_rels/.rels", RELS),
                ("word/document.xml", SRC),
                ("word/media/image1.png", "\u{89}PNG-not-really"),
            ] {
                zip.start_file(name, options).unwrap();
                zip.write_all(content.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }
        buf
    }

    #[test]
    fn dumping_explodes_the_package_and_formats_only_the_xml() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("book.docx");
        std::fs::write(&src, sample_docx()).unwrap();
        let dest = dir.path().join("out");

        let written = do_dump(&src, &dest).unwrap();
        assert!(written.contains(&"word/document.xml".to_string()));

        let doc = std::fs::read_to_string(dest.join("word/document.xml")).unwrap();
        assert!(doc.lines().count() > 3, "was pretty-printed:\n{doc}");

        // The .rels sidecar counts as XML too.
        let rels = std::fs::read_to_string(dest.join("_rels/.rels")).unwrap();
        assert!(rels.starts_with("<?xml"), "got: {rels}");

        // Non-XML parts are left untouched.
        let png = std::fs::read_to_string(dest.join("word/media/image1.png")).unwrap();
        assert_eq!(png, "\u{89}PNG-not-really");
    }

    #[test]
    fn dumping_twice_replaces_the_previous_dump() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("book.docx");
        std::fs::write(&src, sample_docx()).unwrap();
        let dest = dir.path().join("out");

        do_dump(&src, &dest).unwrap();
        std::fs::write(dest.join("stale.txt"), "left over").unwrap();
        do_dump(&src, &dest).unwrap();
        assert!(!dest.join("stale.txt").exists(), "the old dump is cleared");
    }

    #[test]
    fn an_unparseable_part_is_left_alone_rather_than_destroyed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("broken.xml"), "<not <xml").unwrap();
        std::fs::write(dir.path().join("empty.xml"), "").unwrap();
        let done = pretty_all_xml_in_dir(dir.path()).unwrap();
        assert!(done.is_empty());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("broken.xml")).unwrap(),
            "<not <xml"
        );
    }

    #[test]
    fn the_default_destination_matches_the_python_naming() {
        assert_eq!(
            default_dest("/tmp/My Book.docx"),
            PathBuf::from("My Book-dumped")
        );
        assert_eq!(default_dest("book.docx"), PathBuf::from("book-dumped"));
    }
}
