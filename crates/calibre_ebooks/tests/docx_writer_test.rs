//! Write a DOCX to disk through the public API, then read it back with
//! the reader — the two halves of the module were written from the same
//! spec but independently of each other, so agreement between them is
//! worth asserting.

use calibre_ebooks::docx::container::Docx;
use calibre_ebooks::docx::writer::container::{DocxWriter, Margins, PageOptions};
use calibre_ebooks::docx::writer::fonts::{obfuscate_font_data, FontFace, FontsManager};
use calibre_ebooks::docx::writer::xml::Element;
use calibre_ebooks::metadata::meta::MetaInformation;

fn metadata() -> MetaInformation {
    MetaInformation {
        title: "Wuthering Heights".to_string(),
        authors: vec!["Emily Brontë".to_string()],
        languages: vec!["en".to_string()],
        publisher: Some("Thomas Cautley Newby".to_string()),
        tags: vec!["gothic".to_string()],
        ..Default::default()
    }
}

/// Build a package with a paragraph, an image and an embedded font.
fn build() -> (DocxWriter, MetaInformation) {
    let mut writer = DocxWriter::new(PageOptions {
        page_size: "a5".to_string(),
        docx_margins: Margins::uniform(36.0),
        ..Default::default()
    });

    {
        let body = writer.body_mut();
        for text in ["Chapter I", "1801 — I have just returned"] {
            let p = body.append(Element::new("w:p"));
            p.append(Element::new("w:r"))
                .append(Element::new("w:t").with_text(text));
        }
    }

    writer
        .parts
        .insert("word/media/image1.png".to_string(), b"\x89PNG\r\n".to_vec());
    writer.document_relationships.add_image("media/image1.png");

    let face = FontFace {
        family: "Georgia".to_string(),
        weight: 400,
        style: "normal".to_string(),
        data: (0..80u8).collect(),
        source: "georgia.ttf".to_string(),
    };
    let key = [0x11u8; 16];
    let out = FontsManager::new(false).serialize(
        &["Georgia".to_string()],
        std::slice::from_ref(&face),
        &mut writer.font_table,
        &mut writer.embedded_fonts,
        &mut std::iter::once(key),
        &DocxWriter::new(PageOptions::default()).namespace,
    );
    for (name, data) in out.font_data {
        writer.parts.insert(name, data);
    }

    (writer, metadata())
}

#[test]
fn a_written_package_round_trips_through_the_reader() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("wuthering.docx");

    let (writer, mi) = build();
    writer
        .write(std::fs::File::create(&path).expect("create"), &mi)
        .expect("writes");

    let mut docx = Docx::open(&path).expect("the reader accepts it");
    assert_eq!(docx.document_name().unwrap(), "word/document.xml");
    assert!(docx.is_transitional());

    let read = docx.metadata();
    assert_eq!(read.title, "Wuthering Heights");
    assert_eq!(read.authors, vec!["Emily Brontë"]);
    assert_eq!(read.publisher.as_deref(), Some("Thomas Cautley Newby"));
    assert_eq!(read.tags, vec!["gothic"]);
    // "en" canonicalizes to "eng" -- see calibre_utils::localization
    // (issue #140).
    assert_eq!(read.languages, vec!["eng"]);

    // Every part the writer promised in [Content_Types].xml is present.
    for part in [
        "[Content_Types].xml",
        "_rels/.rels",
        "docProps/core.xml",
        "docProps/app.xml",
        "word/document.xml",
        "word/styles.xml",
        "word/numbering.xml",
        "word/fontTable.xml",
        "word/webSettings.xml",
        "word/_rels/document.xml.rels",
        "word/_rels/fontTable.xml.rels",
    ] {
        assert!(docx.exists(part), "missing {part}");
    }

    let body = docx.read_str("word/document.xml").unwrap();
    assert!(body.contains("Chapter I"), "{body}");
    assert!(body.contains("1801 — I have just returned"));

    let rels = docx.document_relationships().unwrap();
    assert_eq!(rels.target("rId5"), Some("word/media/image1.png"));
    assert_eq!(docx.read("word/media/image1.png").unwrap(), b"\x89PNG\r\n");
}

#[test]
fn the_embedded_font_survives_obfuscation_and_the_zip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("fonts.docx");
    let (writer, mi) = build();
    writer
        .write(std::fs::File::create(&path).expect("create"), &mi)
        .expect("writes");

    let mut docx = Docx::open(&path).expect("opens");
    let stored = docx.read("word/fonts/font1.odttf").expect("font part");
    assert_eq!(
        obfuscate_font_data(&stored, &[0x11u8; 16]),
        (0..80u8).collect::<Vec<u8>>(),
        "de-obfuscating with the same key returns the original font"
    );

    let table = docx.read_str("word/fontTable.xml").unwrap();
    assert!(table.contains(r#"w:name="Georgia""#), "{table}");
    assert!(table.contains("w:embedRegular"), "{table}");
    assert_eq!(
        docx.content_type("word/fonts/font1.odttf").as_deref(),
        Some("application/vnd.openxmlformats-officedocument.obfuscatedFont")
    );
}

#[test]
fn writing_twice_produces_identical_bytes() {
    // The timestamp is injected rather than taken from the clock, and
    // relationship ids are handed out in insertion order, so a package
    // is reproducible — which is what makes diffing two conversions
    // meaningful.
    let (writer, mi) = build();
    let mut first = std::io::Cursor::new(Vec::new());
    let mut second = std::io::Cursor::new(Vec::new());
    writer.write(&mut first, &mi).unwrap();
    writer.write(&mut second, &mi).unwrap();
    assert_eq!(first.into_inner(), second.into_inner());
}
